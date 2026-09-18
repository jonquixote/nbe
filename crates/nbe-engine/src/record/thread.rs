//! Dedicated record thread (Prompt 09 WU-pipe, SPEC §9.3).
//!
//! The thread owns the `!Send` handles — the VideoToolbox [`EncodeSession`],
//! the [`RecordingWriter`] (which owns the AudioToolbox AAC converter) — as
//! plain thread-locals. No `Send` needed, no `LocalSet` tricks: the loop hands
//! frames off over a bounded channel and never touches the encoder.
//!
//! ```text
//! loop:  budget pre-check → readback → try_send Frame ──╮
//!                                                      ▼
//! thread: Frame → encode → push_video ──▶ RecordingWriter ──▶ file
//!         tap.drain() → push_audio ──╯        (fragment flushed IMMEDIATELY:
//!                                            prior fragments parseable, AC-6)
//! ```
//!
//! Crash safety is the writer's flush discipline (§9.3 table: ≤1 s fragments,
//! interleaved audio, moov upfront, no finalization required): every fragment
//! is `write() + flush()`ed the moment it completes. Dropping the session
//! mid-take (senders disconnect, no `Stop`) exits WITHOUT finalizing — the
//! SIGKILL shape — and prior fragments still parse.
//!
//! Backpressure policy: [`RECORD_CHANNEL_BOUND`] frames in flight; a full
//! channel sheds (`try_send` fails) and the loop counts the skip. The record
//! path degrades; the View never waits (R4: nothing time-critical on the
//! render pool — the thread is not the render pool either).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use crate::encode::{EncodeSession, EncodedUnit};
use crate::record::session::{force_no_encoder, SessionError};
use crate::record::{AudioTap, RecordError, RecordParams, RecordingWriter};

/// Frames in flight between loop and record thread. 1080p RGBA is ~8.3 MiB,
/// so the bound caps recorder pressure at ~2 frames (~17 MiB); beyond it the
/// handoff sheds and the loop counts the skip. Small on purpose: the thread
/// encodes concurrently, and a deep queue is latency masquerading as
/// throughput.
pub const RECORD_CHANNEL_BOUND: usize = 2;

/// Graceful output shutdown wait (SPEC §16.1 step 3 allows up to 2 seconds;
/// this waits 1500 ms, leaving headroom for the ack pump + WS flush inside
/// that window), shared by `record.stop` and the `show.stop` graceful path.
/// Past it the take is force-abandoned: file kept as-is, warning logged, error
/// surfaces.
pub const RECORD_STOP_TIMEOUT: Duration = Duration::from_millis(1500);

/// Encoder bitrate for the thread-owned session (content path only).
const RECORD_BITRATE: u32 = 8_000_000;

/// Frame-queue poll quantum: the thread drains the audio tap on every wake,
/// so audio interleaving (§9.3) never waits longer than this past a frame.
const DRAIN_POLL: Duration = Duration::from_millis(50);

/// One unit of record work.
#[derive(Debug)]
pub enum RecordMsg {
    /// A View readback from the loop. PTS is encoder-owned
    /// (`frame_index / fps`, strictly increasing per encoded frame): shed
    /// record frames simply never reach the encoder, so the take's timeline
    /// stays continuous — and continuous against the audio timeline, which
    /// counts retained packets the same way.
    Frame { rgba: Vec<u8> },
    /// A pre-encoded unit (TEST SEAM ONLY): feeds the writer without hardware
    /// video, so finish/sidecar/stop paths stay hermetic where VideoToolbox is
    /// absent. Production sends `Frame` exclusively.
    Unit(EncodedUnit),
}

/// Take-level control (unbounded: control is never shed).
#[derive(Debug)]
pub enum ControlMsg {
    /// Finish now: encode nothing further, snapshot markers, finalize the
    /// file + always-sidecar (or fail loudly), report on the done channel.
    Stop,
    /// End now WITHOUT finalizing (`force=true` immediate stop): drop the
    /// writer, keep the file as-is, clear the take's markers, report.
    Abandon,
}

/// The thread's terminal report.
pub type SessionResult = Result<PathBuf, SessionError>;

/// Everything the thread needs, moved in at spawn.
pub struct ThreadArgs {
    pub params: RecordParams,
    pub initial_sets: Option<(Vec<u8>, Vec<u8>)>,
    pub tap: Arc<AudioTap>,
    pub frame_rx: Receiver<RecordMsg>,
    pub control_rx: Receiver<ControlMsg>,
    pub done_tx: Sender<SessionResult>,
    /// Thread-side sheds (failed encoder open, refused encodes) land here —
    /// the same engine-state counter the loop feeds, so no skip is invisible.
    pub skipped: Arc<std::sync::atomic::AtomicU64>,
}

/// Spawn the record thread (detached: the done channel is the rendezvous, the
/// handle is reaped on the graceful path and dropped on the force path).
pub fn spawn_record_thread(args: ThreadArgs) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("nbe-record".into())
        .spawn(move || run_thread(args))
        .expect("the record thread must start")
}

fn run_thread(args: ThreadArgs) {
    let result = drive(args);
    // The receiver may be gone (abandoned/dropped take): the file is already
    // kept as-is by construction, so a failed report is not a second failure.
    // (Explicit `let _` — clippy::let_underscore_must_use is not enabled, but
    // say it anyway.)
    let _ = result.1.send(result.0);
}

/// Bounded wait for the thread's terminal report. `Timeout` maps to the force
/// path (file kept as-is, warning + error upstream); `Disconnected` means the
/// thread is gone without reporting (panic or crash-shape exit after our
/// senders dropped) — the take's outcome is likewise unknown, so it maps to
/// the same [`SessionError::Timeout`] force path (file kept as-is), never to
/// a different token. Both are errors, never a silent ack.
pub fn await_done(
    rx: &Receiver<SessionResult>,
    timeout: Duration,
) -> Result<PathBuf, SessionError> {
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(SessionError::Timeout(format!(
            "record thread did not finish within {} ms; file kept as-is",
            timeout.as_millis()
        ))),
        Err(RecvTimeoutError::Disconnected) => Err(SessionError::Timeout(
            "record thread gone without reporting; file kept as-is".into(),
        )),
    }
}

struct Drive {
    params: RecordParams,
    output_path: PathBuf,
    sets: Option<(Vec<u8>, Vec<u8>)>,
    tap: Arc<AudioTap>,
    frame_rx: Receiver<RecordMsg>,
    control_rx: Receiver<ControlMsg>,
    skipped: Arc<std::sync::atomic::AtomicU64>,
    encoder: Option<EncodeSession>,
    /// Eager-open refusal (SPEC `E_NO_HARDWARE_ENCODER` shape): `Frame`s shed
    /// from here on; `Unit`s still flow (the hermetic seam); the stop reports
    /// [`SessionError::NoEncoder`] when nothing was recorded.
    setup_err: Option<String>,
    writer: Option<RecordingWriter>,
    video_units: u64,
    keyframe_seen: bool,
}

fn drive(args: ThreadArgs) -> (SessionResult, Sender<SessionResult>) {
    let ThreadArgs {
        params,
        initial_sets,
        tap,
        frame_rx,
        control_rx,
        done_tx,
        skipped,
    } = args;
    let output_path = params
        .directory
        .join(crate::record::recording_filename(&params));
    let mut d = Drive {
        params,
        output_path,
        sets: initial_sets,
        tap,
        frame_rx,
        control_rx,
        skipped,
        encoder: None,
        setup_err: None,
        writer: None,
        video_units: 0,
        keyframe_seen: false,
    };
    // Eager open, off the loop: a missing encoder is known before the first
    // frame rather than discovered mid-take. The forced-unavailable seam
    // behaves exactly like missing hardware without an open attempt.
    if force_no_encoder() {
        d.setup_err = Some("hardware encoder forced unavailable (test seam)".into());
    } else {
        let p = &d.params;
        match EncodeSession::open(p.width, p.height, p.fps, RECORD_BITRATE) {
            Ok(enc) => d.encoder = Some(enc),
            Err(e) => d.setup_err = Some(e.to_string()),
        }
    }
    let result = d.run();
    (result, done_tx)
}

impl Drive {
    fn run(&mut self) -> SessionResult {
        loop {
            // Frames first: a pending `Stop` waits behind already-handed-off
            // frames (no handed-off frame is lost at the boundary), while a
            // live loop keeps the queue non-empty and control responsive via
            // the poll below.
            match self.frame_rx.try_recv() {
                Ok(msg) => {
                    self.handle_msg(msg)?;
                    self.drain_tap_fatal()?;
                    continue;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return self.on_frames_gone(),
            }
            match self.control_rx.try_recv() {
                Ok(ControlMsg::Stop) => return self.finish_take(),
                Ok(ControlMsg::Abandon) => return self.abandon_take(),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    // Control gone with frames possibly pending: fall through
                    // to the blocking wait, which resolves via frames-gone.
                }
            }
            match self.frame_rx.recv_timeout(DRAIN_POLL) {
                Ok(msg) => {
                    self.handle_msg(msg)?;
                    self.drain_tap_fatal()?;
                }
                Err(RecvTimeoutError::Timeout) => {
                    // No video this quantum: still drain, so audio
                    // interleaving never stalls past a frame boundary.
                    self.drain_tap_fatal()?;
                }
                Err(RecvTimeoutError::Disconnected) => return self.on_frames_gone(),
            }
        }
    }

    /// Fail-fast helper: writer push failures end the take loudly HERE (the
    /// file keeps prior flushed fragments as-is); the stop surfaces the
    /// report instead of hanging a broken take.
    fn drain_tap_fatal(&mut self) -> Result<(), SessionError> {
        if let Err(e) = self.drain_tap() {
            return Err(SessionError::Record(e));
        }
        Ok(())
    }

    fn handle_msg(&mut self, msg: RecordMsg) -> Result<(), SessionError> {
        match msg {
            RecordMsg::Frame { rgba } => {
                let Some(enc) = self.encoder.as_mut() else {
                    // No encoder (failed open): shed the frame, count it, keep
                    // the take alive for `Unit`s / a loud stop.
                    self.count_skip();
                    return Ok(());
                };
                match enc.encode_rgba(&rgba) {
                    Ok(units) => {
                        self.capture_sets();
                        for u in &units {
                            self.push_video(u)?;
                        }
                    }
                    Err(_) => {
                        // Hardware encode is all-or-nothing by design (no CPU
                        // fallback): a refusal degrades the frame, never the
                        // take. The encoder stays open; the next frame retries.
                        self.count_skip();
                    }
                }
            }
            RecordMsg::Unit(u) => {
                self.push_video(&u)?;
            }
        }
        Ok(())
    }

    /// First keyframe exposes the stream's real sets: capture them for the
    /// writer's `avcC` (first capture wins; sets are stream parameters).
    fn capture_sets(&mut self) {
        if self.sets.is_none() {
            if let Some((sps, pps)) = self.encoder.as_ref().and_then(|e| e.parameter_sets()) {
                self.sets = Some((sps.clone(), pps.clone()));
                if let Some(w) = self.writer.as_mut() {
                    // The header only writes on the first IDR push, which has
                    // not happened yet (no sets existed before this capture),
                    // so this cannot be late.
                    let _ = w.set_parameter_sets(sps, pps);
                }
            }
        }
    }

    fn ensure_writer(&mut self) -> Result<(), SessionError> {
        if self.writer.is_none() {
            let mut params = self.params.clone();
            if let Some((sps, pps)) = &self.sets {
                params.sps = sps.clone();
                params.pps = pps.clone();
            }
            // The file materializes on first content, not at start: a take
            // with zero frames leaves no empty file behind.
            self.writer = Some(RecordingWriter::create(&params)?);
        }
        Ok(())
    }

    fn push_video(&mut self, unit: &EncodedUnit) -> Result<(), SessionError> {
        self.ensure_writer()?;
        let w = self.writer.as_mut().expect("writer ensured above");
        w.push_video(unit).map_err(SessionError::Record)?;
        self.video_units += 1;
        self.keyframe_seen |= unit.is_keyframe;
        Ok(())
    }

    /// Drain the shared tap into the writer. No writer yet (no video) means
    /// no AAC home: the tap keeps buffering (bounded, drop-oldest, counted —
    /// a late writer loses the past, never the present).
    fn drain_tap(&mut self) -> Result<(), RecordError> {
        let Some(w) = self.writer.as_mut() else {
            return Ok(());
        };
        let drained = self.tap.drain();
        if !drained.is_empty() {
            w.push_audio(&drained)?;
        }
        Ok(())
    }

    fn count_skip(&self) {
        self.skipped
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Frames disconnected: the session was dropped without `Stop` (the
    /// SIGKILL shape at pipeline level). Honor a queued `Stop` (finish), else
    /// exit WITHOUT finalizing — prior flushed fragments stay parseable, no
    /// sidecar, no report expected.
    fn on_frames_gone(&mut self) -> SessionResult {
        match self.control_rx.try_recv() {
            Ok(ControlMsg::Stop) => self.finish_take(),
            // Abandon or nothing: the crash-shape exit. Markers are left for
            // the take boundary that never came (next start/load clears).
            _ => Err(SessionError::Record(RecordError::Input(
                "record pipeline dropped mid-take; prior fragments kept as-is".into(),
            ))),
        }
    }

    /// Graceful finish: snapshot markers, finalize file + always-sidecar (or
    /// fail loudly), clear the take's markers, report.
    fn finish_take(&mut self) -> SessionResult {
        // Drain the tail audio the take actually captured before finalizing.
        if let Some(w) = self.writer.as_mut() {
            let drained = self.tap.drain();
            if !drained.is_empty() {
                w.push_audio(&drained).map_err(SessionError::Record)?;
            }
        }
        if self.video_units == 0 {
            crate::record::markers::clear();
            if let Some(why) = self.setup_err.take() {
                return Err(SessionError::NoEncoder(why));
            }
            return Err(SessionError::Record(RecordError::Input(
                "no video units".into(),
            )));
        }
        if !self.keyframe_seen {
            crate::record::markers::clear();
            return Err(SessionError::Record(RecordError::Input(
                "no keyframe observed; refusing undecodable recording".into(),
            )));
        }
        let Some(w) = self.writer.take() else {
            crate::record::markers::clear();
            return Err(SessionError::Record(RecordError::Input(
                "no video units".into(),
            )));
        };
        // `finish` snapshots the marker store into the always-sidecar; the
        // take ends here either way.
        let result = w.finish().map_err(SessionError::Record);
        crate::record::markers::clear();
        result
    }

    /// Immediate end (`force=true`): no finish, no sidecar — the file keeps
    /// whatever fragments flushed, as-is. The take still ends (markers out).
    fn abandon_take(&mut self) -> SessionResult {
        crate::record::markers::clear();
        Ok(self.output_path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn await_done_maps_timeout_to_force_path_error() {
        let (_tx, rx) = std::sync::mpsc::channel::<SessionResult>();
        // No message will ever arrive: the wait must expire, loudly.
        let err = await_done(&rx, Duration::from_millis(10)).unwrap_err();
        assert!(
            matches!(err, SessionError::Timeout(_)),
            "timeout must surface as SessionError::Timeout, got: {err}"
        );
        assert!(
            err.to_string().contains("kept as-is"),
            "timeout error must promise the file is kept, got: {err}"
        );
    }

    #[test]
    fn await_done_maps_gone_thread_to_timeout_never_silence() {
        let (tx, rx) = std::sync::mpsc::channel::<SessionResult>();
        drop(tx);
        // A dead thread reports nothing, so the outcome is unknown — the same
        // force path as an expired wait (file kept as-is), not Input.
        let err = await_done(&rx, Duration::from_secs(5)).unwrap_err();
        assert!(
            matches!(err, SessionError::Timeout(_)),
            "a gone thread must surface as SessionError::Timeout, got: {err}"
        );
        assert!(
            err.to_string().contains("E_RECORD_TIMEOUT"),
            "gone-thread error must carry the timeout token, got: {err}"
        );
        assert!(
            err.to_string().contains("kept as-is"),
            "gone-thread error must promise the file is kept, got: {err}"
        );
    }

    #[test]
    fn await_done_passes_through_ok_and_err() {
        let (tx, rx) = std::sync::mpsc::channel::<SessionResult>();
        tx.send(Ok(PathBuf::from("/tmp/take.mp4"))).unwrap();
        assert_eq!(
            await_done(&rx, Duration::from_secs(1)).unwrap(),
            PathBuf::from("/tmp/take.mp4")
        );

        let (tx, rx) = std::sync::mpsc::channel::<SessionResult>();
        tx.send(Err(SessionError::NoEncoder("nope".into())))
            .unwrap();
        let err = await_done(&rx, Duration::from_secs(1)).unwrap_err();
        assert!(err.to_string().contains("E_NO_HARDWARE_ENCODER"));
    }
}
