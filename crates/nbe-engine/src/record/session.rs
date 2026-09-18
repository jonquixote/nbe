//! Record session glue (Prompt 09 WU-pipe, SPEC §16.14).
//!
//! Lifecycle owner between the directive path and the record thread:
//! `record.start` opens a [`RecordSession`] (spawning the thread),
//! `record.stop` / `show.stop` quiescence ends it with a bounded wait for the
//! thread's terminal report BEFORE `apply()` emits the ack (SPEC §5.9.5: the
//! ack is honest only after the effect is real).
//!
//! ## Why plain data on this side (no live encoder/writer held)
//!
//! The live recording objects are `!Send`: `EncodeSession` holds a
//! `CFRetained<VTCompressionSession>` and `RecordingWriter` holds an
//! `AacEncoder` (`*mut OpaqueAudioConverter`), and `EngineState` must stay
//! `Send + Sync`. The dedicated record thread owns those handles as
//! thread-locals ([`thread`](crate::record::thread)); this side holds only
//! channel endpoints + naming, all `Send`. No buffers live here: frames flow
//! loop → bounded channel → thread → file, and fragments flush immediately, so
//! there is nothing to cap and no one-shot `finish` over memory (the old
//! `MAX_PENDING_*` caps died with the buffers; a SIGKILL costs at most the
//! in-progress tail, AC-6).
//!
//! ## Payload contract (SPEC §16.14)
//!
//! `record.start` carries `{ outputId? }` and nothing else. Show, episode,
//! geometry, and rate derive from the loaded package + engine (see
//! `on_record_start`): the retired `show`/`episode`/`width`/`height`/`fps`/
//! `startTimestamp` payload fields are NOT read — extra fields are ignored,
//! never rejected, so an older control plane keeps working while its naming
//! fields stop mattering. `outputId` selects the (single) configured record
//! target and names the episode filename component.
//!
//! ## Defined behavior
//!
//! * [`set_force_no_encoder`] forces the unavailable path exactly as a
//!   machine with no hardware encoder behaves — consulted by the start probe
//!   and the thread's eager open, never a CPU fallback.
//! * [`RecordSession::open_with_sets`] is the hardware-free seam for tests:
//!   caller-supplied sets + [`thread::RecordMsg::Unit`] feed the writer with
//!   no VideoToolbox touched (AAC is still required at finish, when the real
//!   writer runs).
//! * A second `record.start` while `Recording` is refused upstream with
//!   `E_FORBIDDEN_STATE` and preserves the live pipeline; the chapter list
//!   still resets (every start attempt while running opens a fresh take).
//! * Stopping with no frames at all is `E_RECORD_INPUT` — loudly, never an
//!   empty file masquerading as a recording. Finishing before any keyframe is
//!   likewise `E_RECORD_INPUT`.
//! * Marker discipline: cleared on start, on load, and on every take end the
//!   thread finalizes or abandons (the thread snapshots before clearing, so a
//!   graceful stop's sidecar is ordered); the directive path clears again on
//!   the graceful/immediate stop paths (idempotent). The graceful-TIMEOUT path
//!   deliberately does NOT clear: the detached take's late finish owns its
//!   snapshot, and the next start/load clears anyway.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use crate::record::thread::{
    await_done, spawn_record_thread, ControlMsg, RecordMsg, SessionResult, ThreadArgs,
    RECORD_CHANNEL_BOUND,
};
use crate::record::{AudioTap, RecordError, RecordParams};

/// Forced-unavailable seam (tests only): when set, the start probe and the
/// thread's eager open treat the hardware encoder as absent without touching
/// VideoToolbox.
static FORCE_NO_ENCODER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Force (or release) the no-encoder path. Test seam only.
pub fn set_force_no_encoder(force: bool) {
    FORCE_NO_ENCODER.store(force, std::sync::atomic::Ordering::SeqCst);
}

/// The seam state: true while forced-unavailable.
pub fn force_no_encoder() -> bool {
    FORCE_NO_ENCODER.load(std::sync::atomic::Ordering::SeqCst)
}

/// True unless the seam forces otherwise and a hardware H.264 encoder answers.
/// The start path probes this WITHOUT opening a stream (the recording encoder
/// opens later, on the record thread, at the first take).
pub fn encoder_available() -> bool {
    if FORCE_NO_ENCODER.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    crate::encode::is_available()
}

/// Opening or ending a record take fails loudly, with stable tokens.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// No hardware encoder (seam-forced or genuinely absent): `E_NO_HARDWARE_ENCODER`.
    #[error("E_NO_HARDWARE_ENCODER: {0}")]
    NoEncoder(String),
    /// The thread did not report within the bounded wait:
    /// `E_RECORD_TIMEOUT` (engine-local token, not spec — the spec's stop
    /// failure modes predate the threaded take). File kept as-is, take
    /// force-abandoned, error surfaces (never a silent ack).
    #[error("E_RECORD_TIMEOUT: {0}")]
    Timeout(String),
    /// Writer/file failure underneath (carries `E_DISK` / `E_AAC_UNAVAILABLE` /
    /// `E_RECORD_INPUT` verbatim).
    #[error(transparent)]
    Record(#[from] RecordError),
}

/// An open recording: the reserved output path, the shared audio tap, and the
/// pipeline endpoints. Naming/geometry live only in the reserved filename
/// (computed at open); nothing reads them back, so they are not stored.
/// Deliberately `Send` (channels + plain data) so engine state can hold it;
/// the `!Send` half lives on the record thread.
pub struct RecordSession {
    output_path: PathBuf,
    tap: Arc<AudioTap>,
    frame_tx: SyncSender<RecordMsg>,
    control_tx: Sender<ControlMsg>,
    done_rx: Option<Receiver<SessionResult>>,
    handle: Option<std::thread::JoinHandle<()>>,
    finished: bool,
}

impl RecordSession {
    /// Open a take in `directory`: validates the path (creates it), reserves
    /// the output filename, publishes nothing yet, and spawns the record
    /// thread. The handler stays non-blocking: the spawn hands the thread its
    /// moved args and returns; the encoder opens on the thread.
    ///
    /// Many args because they are independent facts (naming, geometry, rate,
    /// tap, skip counter); bundling them would just move the struct boundary
    /// without removing a field.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        directory: &Path,
        show: &str,
        episode: &str,
        start_timestamp: &str,
        width: u32,
        height: u32,
        fps: u32,
        tap: Arc<AudioTap>,
        skipped: Arc<std::sync::atomic::AtomicU64>,
    ) -> Result<Self, RecordError> {
        Self::open_inner(
            directory,
            show,
            episode,
            start_timestamp,
            width,
            height,
            fps,
            tap,
            skipped,
            None,
        )
    }

    /// Hardware-free seam (tests only): like [`open`](Self::open) but the
    /// thread starts with caller-supplied parameter sets, so
    /// [`RecordMsg::Unit`] feeds finalize without VideoToolbox. `finish` still
    /// runs the real writer (AAC required there).
    #[allow(clippy::too_many_arguments)]
    pub fn open_with_sets(
        directory: &Path,
        show: &str,
        episode: &str,
        start_timestamp: &str,
        width: u32,
        height: u32,
        fps: u32,
        tap: Arc<AudioTap>,
        skipped: Arc<std::sync::atomic::AtomicU64>,
        sets: (Vec<u8>, Vec<u8>),
    ) -> Result<Self, RecordError> {
        Self::open_inner(
            directory,
            show,
            episode,
            start_timestamp,
            width,
            height,
            fps,
            tap,
            skipped,
            Some(sets),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_inner(
        directory: &Path,
        show: &str,
        episode: &str,
        start_timestamp: &str,
        width: u32,
        height: u32,
        fps: u32,
        tap: Arc<AudioTap>,
        skipped: Arc<std::sync::atomic::AtomicU64>,
        initial_sets: Option<(Vec<u8>, Vec<u8>)>,
    ) -> Result<Self, RecordError> {
        std::fs::create_dir_all(directory).map_err(|e| RecordError::Disk(e.to_string()))?;
        let params = RecordParams {
            directory: directory.to_path_buf(),
            show: show.to_string(),
            episode: episode.to_string(),
            start_timestamp: start_timestamp.to_string(),
            width,
            height,
            fps,
            sps: Vec::new(),
            pps: Vec::new(),
        };
        let output_path = directory.join(crate::record::recording_filename(&params));
        let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel(RECORD_CHANNEL_BOUND);
        let (control_tx, control_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = spawn_record_thread(ThreadArgs {
            params,
            initial_sets,
            tap: tap.clone(),
            frame_rx,
            control_rx,
            done_tx,
            skipped,
        });
        Ok(Self {
            output_path,
            tap,
            frame_tx,
            control_tx,
            done_rx: Some(done_rx),
            handle: Some(handle),
            finished: false,
        })
    }

    /// The reserved output path. The file materializes on first content (not
    /// at start): a take with zero frames leaves no empty file behind.
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    /// The take's shared audio tap (published to the audio driver at start;
    /// drained by the record thread).
    pub fn tap(&self) -> Arc<AudioTap> {
        self.tap.clone()
    }

    /// The loop's handoff endpoint (cloned per handoff; `try_send` only —
    /// a full channel sheds, never blocks).
    pub fn frame_sender(&self) -> SyncSender<RecordMsg> {
        self.frame_tx.clone()
    }

    /// True once the engine ended the take (graceful report received, or
    /// abandoned). False while live and after a timed-out wait (outcome
    /// unknown — the take detached).
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// End the take gracefully: signal `Stop`, wait up to `timeout` for the
    /// thread's terminal report (file + sidecar complete BEFORE this returns —
    /// the ack that follows is honest), reap the thread. Timeout takes the
    /// force path: `Abandon` is sent (file kept as-is), the thread detached,
    /// and [`SessionError::Timeout`] surfaces — never a silent ack.
    pub fn stop_and_finish(&mut self, timeout: Duration) -> Result<PathBuf, SessionError> {
        let rx = self.done_rx.take().expect("stop_and_finish called twice");
        // Unbounded control: the signal is never shed. If the thread is
        // already gone the send fails and the wait below reports it.
        let _ = self.control_tx.send(ControlMsg::Stop);
        match await_done(&rx, timeout) {
            Ok(path) => {
                self.finished = true;
                self.join_thread();
                Ok(path)
            }
            Err(SessionError::Timeout(msg)) => {
                // Force path: end the take without finalizing. The file keeps
                // whatever fragments flushed, as-is.
                let _ = self.control_tx.send(ControlMsg::Abandon);
                self.detach_thread();
                Err(SessionError::Timeout(msg))
            }
            Err(e) => {
                self.finished = true;
                self.join_thread();
                Err(e)
            }
        }
    }

    /// End the take WITHOUT finalizing (`force=true` immediate stop): the
    /// file keeps flushed fragments as-is, no sidecar, take over. Queued
    /// control is best-effort — a thread that already exited needs nothing.
    pub fn abandon(&mut self) {
        let _ = self.control_tx.send(ControlMsg::Abandon);
        self.finished = true;
        self.detach_thread();
        self.done_rx.take();
    }

    fn join_thread(&mut self) {
        if let Some(h) = self.handle.take() {
            // The report arrived, so the thread exited: joining cannot block.
            let _ = h.join();
        }
    }

    fn detach_thread(&mut self) {
        // Dropping the handle detaches: the thread keeps (or ends) its take
        // on its own; the file stays as-is by construction.
        self.handle.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_seam_reports_unavailable_without_touching_hardware() {
        set_force_no_encoder(true);
        assert!(!encoder_available());
        set_force_no_encoder(false);
    }
}
