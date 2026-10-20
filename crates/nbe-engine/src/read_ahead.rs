//! A plain clip's read-ahead decoder (SPEC §12.8.1, v0.4.8 row 12).
//!
//! One worker thread per streaming clip decodes its frames sequentially, from
//! a start index to the clip's end, and hands each one to the render thread
//! over a bounded channel. The worker never decides which frame shows (the
//! master clock does, §12.1) and never reads a clock of its own: it is paced
//! by occupancy alone. A full channel blocks it, and the render thread drains
//! the channel only into window slots that are free (rule 3). So at most
//! `CHANNEL_DEPTH` frames, plus the one the render thread holds back, are ever
//! decoded past the window.
//!
//! The decode session is opened on the worker's own thread: an
//! `AVAssetReader` is not `Send`, and nothing here moves one between threads.
//! Dropping a `ReadAhead` closes the channel, and the worker's next send fails
//! and it exits. It is never joined, so a drop never blocks the render thread.

use crate::decode::{DecodeSession, DecodedFrame};
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};

/// Frames the worker may have decoded and queued ahead of the render thread.
pub const CHANNEL_DEPTH: usize = 2;

/// A running read-ahead decoder for one clip.
#[derive(Debug)]
pub struct ReadAhead {
    rx: Receiver<DecodedFrame>,
    /// The index of the next frame this worker will deliver.
    next: u64,
    /// The worker has delivered its last frame, or failed.
    done: bool,
}

/// What the render thread got from the worker on one poll.
#[derive(Debug)]
pub enum Delivery {
    Frame(DecodedFrame),
    /// Nothing ready yet: the worker is decoding, or blocked on a full window.
    Pending,
    /// The worker has delivered every frame it will (its end, or a failure).
    Done,
}

impl ReadAhead {
    /// Start decoding `path` at frame `start`, up to (not including) `end`.
    /// Frames before `start` are decoded for their timestamps only and never
    /// copied (`next_frame_meta`).
    pub fn spawn(asset_id: &str, path: PathBuf, start: u64, end: u64) -> Self {
        let (tx, rx) = sync_channel::<DecodedFrame>(CHANNEL_DEPTH);
        let name = format!("read-ahead {asset_id}");
        let asset = asset_id.to_string();
        let spawned = std::thread::Builder::new().name(name).spawn(move || {
            let mut session = match DecodeSession::open(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(asset = %asset, err = %e, "read-ahead could not open the clip");
                    return;
                }
            };
            let mut index = 0u64;
            while index < start {
                match session.next_frame_meta() {
                    Ok(Some(_)) => index += 1,
                    Ok(None) => return,
                    Err(e) => {
                        tracing::warn!(asset = %asset, err = %e, "read-ahead skip failed");
                        return;
                    }
                }
            }
            while index < end {
                match session.next_frame() {
                    // The render thread closed the channel: this read-ahead was
                    // dropped (re-positioned, shed, or the package unloaded).
                    Ok(Some(f)) => {
                        if tx.send(f).is_err() {
                            return;
                        }
                        index += 1;
                    }
                    Ok(None) => return,
                    Err(e) => {
                        tracing::warn!(asset = %asset, err = %e, "read-ahead decode failed");
                        return;
                    }
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!(asset = %asset_id, err = %e, "read-ahead thread did not start");
        }
        Self {
            rx,
            next: start,
            done: false,
        }
    }

    /// The index of the next frame this worker will deliver.
    pub fn next(&self) -> u64 {
        self.next
    }

    /// Poll for the next frame without blocking: the render thread's only way
    /// to read the worker (rule 4: it never waits on the decoder).
    pub fn poll(&mut self) -> Delivery {
        if self.done {
            return Delivery::Done;
        }
        match self.rx.try_recv() {
            Ok(f) => {
                self.next = f.index + 1;
                Delivery::Frame(f)
            }
            Err(TryRecvError::Empty) => Delivery::Pending,
            Err(TryRecvError::Disconnected) => {
                self.done = true;
                Delivery::Done
            }
        }
    }
}
