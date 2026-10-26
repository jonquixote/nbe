//! The device sink's sample ring (device-audio design D2, option A).
//!
//! The audio thread renders on the master clock and pushes; the device
//! callback only copies out. One producer, one consumer, preallocated at
//! open. Samples are stored as `AtomicU32` bit patterns, so the ring needs no
//! `unsafe` (the workspace denies it) and every access on the callback's side
//! is a plain atomic load or store: no lock, no allocation, nothing that can
//! block (SPEC §8.9).
//!
//! The two counters are monotonic sample counts, never wrapped indices, so a
//! full ring and an empty one cannot be confused and a reader can tell exactly
//! how many frames have ever passed through.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::audio::CHANNELS;

struct Shared {
    buf: Box<[AtomicU32]>,
    /// Samples ever pushed. Written only by the producer.
    written: AtomicU64,
    /// Samples ever popped. Written only by the consumer.
    read: AtomicU64,
}

/// The audio thread's end.
pub struct RingProducer {
    shared: Arc<Shared>,
}

/// The device callback's end.
pub struct RingConsumer {
    shared: Arc<Shared>,
}

/// A ring holding `capacity_frames` interleaved frames.
pub fn ring(capacity_frames: usize) -> (RingProducer, RingConsumer) {
    let buf: Box<[AtomicU32]> = (0..capacity_frames.max(1) * CHANNELS)
        .map(|_| AtomicU32::new(0))
        .collect();
    let shared = Arc::new(Shared {
        buf,
        written: AtomicU64::new(0),
        read: AtomicU64::new(0),
    });
    (
        RingProducer {
            shared: shared.clone(),
        },
        RingConsumer { shared },
    )
}

impl Shared {
    fn capacity(&self) -> u64 {
        self.buf.len() as u64
    }
}

impl RingProducer {
    /// Push as many of `samples` as fit, whole frames only, and return how
    /// many samples went in. The driver keeps the fill at its target, so a
    /// short push means the device has stopped consuming.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let s = &*self.shared;
        let w = s.written.load(Ordering::Relaxed);
        let r = s.read.load(Ordering::Acquire);
        let free = s.capacity() - (w - r);
        let n = (samples.len() as u64).min(free) as usize / CHANNELS * CHANNELS;
        let cap = s.capacity();
        for (i, v) in samples[..n].iter().enumerate() {
            s.buf[((w + i as u64) % cap) as usize].store(v.to_bits(), Ordering::Relaxed);
        }
        s.written.store(w + n as u64, Ordering::Release);
        n
    }

    /// Frames waiting for the device.
    pub fn fill_frames(&self) -> u64 {
        let s = &*self.shared;
        (s.written.load(Ordering::Relaxed) - s.read.load(Ordering::Acquire)) / CHANNELS as u64
    }

    /// Frames ever pushed: the ring index the next pushed frame takes.
    pub fn written_frames(&self) -> u64 {
        self.shared.written.load(Ordering::Relaxed) / CHANNELS as u64
    }

    pub fn capacity_frames(&self) -> u64 {
        self.shared.capacity() / CHANNELS as u64
    }
}

impl RingConsumer {
    /// Copy up to `out.len()` samples out, whole frames only, and return how
    /// many were copied. The caller silences the rest: an empty ring is an
    /// underrun, and it must sound like silence, never like stale samples.
    pub fn pop_into(&mut self, out: &mut [f32]) -> usize {
        let s = &*self.shared;
        let r = s.read.load(Ordering::Relaxed);
        let w = s.written.load(Ordering::Acquire);
        let n = (out.len() as u64).min(w - r) as usize / CHANNELS * CHANNELS;
        let cap = s.capacity();
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = f32::from_bits(s.buf[((r + i as u64) % cap) as usize].load(Ordering::Relaxed));
        }
        s.read.store(r + n as u64, Ordering::Release);
        n
    }

    /// Whether the producer has ever pushed: before its first block, the
    /// device plays the silence of a stream that has not started, not an
    /// underrun.
    pub fn started(&self) -> bool {
        self.shared.written.load(Ordering::Acquire) > 0
    }

    /// Frames ever popped: the ring index of the next frame the device plays.
    pub fn read_frames(&self) -> u64 {
        self.shared.read.load(Ordering::Relaxed) / CHANNELS as u64
    }
}
