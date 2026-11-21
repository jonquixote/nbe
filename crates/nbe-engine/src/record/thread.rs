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
//!
//! **On the zero-copy path that policy moves earlier rather than changing.**
//! Shedding is free for `Frame { rgba }`, which owns its copy, and impossible
//! for `Surface`, which is a loan of one mutable allocation: a surface already
//! drawn into cannot be dropped without dropping a frame the encoder may still
//! be reading. So the question — *is there room?* — is asked before the draw,
//! as *is a free surface available?*, and a `None` there is the same skip,
//! counted the same way ([`crate::record::feed`],
//! `docs/zero-copy-p3-design.md` Q3).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::encode::{EncodeSession, EncodedUnit};
use crate::record::align::TakeAudio;
use crate::record::session::{force_no_encoder, SessionError};
use crate::record::{AudioTap, CountSkip, RecordError, RecordParams, RecordingWriter, SkipPath};

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
    Frame {
        rgba: Vec<u8>,
        /// The master frame the View drew, when the loop knows it (the record
        /// trace's join; `None` from callers that do not).
        frame: Option<u64>,
    },
    /// A shared IOSurface the compositor drew into, handed over WITHOUT a
    /// copy (ZERO-COPY Phase 3b). The `Arc` is the pool's loan: dropping it
    /// here is what returns the surface to the free list, so the encoder
    /// finishing with a frame is the same event as the compositor being
    /// allowed to draw the next one into it.
    Surface {
        surface: Arc<nbe_decode::zerocopy::SharedSurface>,
        /// R31: the View draw into `surface`. The thread waits on it before
        /// the encoder reads the surface.
        drawn: crate::record::DrawDone,
        /// The master frame the View drew (the record trace's join).
        frame: u64,
    },
    /// A pre-encoded unit (TEST SEAM ONLY): feeds the writer without hardware
    /// video, so finish/sidecar/stop paths stay hermetic where VideoToolbox is
    /// absent. Production sends `Frame` or `Surface` — never this.
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
    pub skipped: Arc<dyn CountSkip>,
    /// Set when the thread ends without a `Stop` or `Abandon` (a failed write,
    /// a panic): why, in words. The session reads it while the take is still
    /// believed running, so telemetry can say the take failed
    /// ([`RecordSession::ended_on_its_own`](crate::record::RecordSession::ended_on_its_own)).
    pub ended_on_its_own: Arc<Mutex<Option<String>>>,
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
    // A panic anywhere below unwinds through this guard, which notes it: a
    // thread that dies records nothing more, and the operator must hear it
    // before the stop, not at it.
    let mut guard = EndedGuard {
        note: args.ended_on_its_own.clone(),
        armed: true,
    };
    let (result, done_tx, asked) = drive(args);
    guard.armed = false;
    if !asked {
        let reason = match &result {
            Err(e) => e.to_string(),
            Ok(path) => format!(
                "the take's frames stopped arriving; {} kept as written",
                path.display()
            ),
        };
        tracing::error!(
            reason = %reason,
            "record: the take ended on its own; nothing more is recorded until it is stopped"
        );
        *guard.note.lock().unwrap_or_else(|p| p.into_inner()) = Some(reason);
    }
    // The receiver may be gone (abandoned/dropped take): the file is already
    // kept as-is by construction, so a failed report is not a second failure.
    // (Explicit `let _` — clippy::let_underscore_must_use is not enabled, but
    // say it anyway.)
    let _ = done_tx.send(result);
}

/// Notes a panic on the record thread for the session to read.
struct EndedGuard {
    note: Arc<Mutex<Option<String>>>,
    armed: bool,
}

impl Drop for EndedGuard {
    fn drop(&mut self) {
        if self.armed && std::thread::panicking() {
            tracing::error!(
                "record: the record thread panicked; nothing more is recorded until the take is stopped"
            );
            *self.note.lock().unwrap_or_else(|p| p.into_inner()) =
                Some("the record thread panicked; the file keeps what was written before".into());
        }
    }
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
    skipped: Arc<dyn CountSkip>,
    encoder: Option<EncodeSession>,
    /// Eager-open refusal (SPEC `E_NO_HARDWARE_ENCODER` shape): `Frame`s shed
    /// from here on; `Unit`s still flow (the hermetic seam); the stop reports
    /// [`SessionError::NoEncoder`] when nothing was recorded.
    setup_err: Option<String>,
    writer: Option<RecordingWriter>,
    video_units: u64,
    keyframe_seen: bool,
    /// `NBE_RECORD_TRACE`: one line per handed-off frame (R31's probe).
    trace: Option<RecordTrace>,
    /// The take on the master clock: the master frame the video's PTS count
    /// from (the first frame handed off with one), and the first and last
    /// master frames the encoder took. A frame the take skipped is then a
    /// held frame, not a shift of everything after it (the stream path's rule,
    /// `stream.rs` `encode_video`).
    base_frame: Option<u64>,
    first_frame: Option<u64>,
    last_frame: Option<u64>,
    /// The take's audio, cut to the video's span (`align.rs`). `None` until
    /// the first video frame is encoded, and for a take whose frames or tap
    /// carry no master position (the hermetic seams): those keep the old
    /// behaviour, everything the tap held.
    audio: Option<TakeAudio>,
    /// A `Stop` or `Abandon` ended the take; otherwise it ended on its own.
    asked_to_end: bool,
}

/// How long a stop waits for the audio driver to push the take's last frame
/// into the tap. The driver is normally a block or two behind real time; past
/// this the end is padded with silence and the line says how much.
const AUDIO_TAIL_WAIT: Duration = Duration::from_millis(300);

/// The master audio frame where master video frame `frame` begins (the graph's
/// own mapping, `AudioGraph::sample_for_master_frame` with no offset).
fn audio_frame(frame: u64, fps: u32) -> i64 {
    (frame as i64) * i64::from(crate::audio::SAMPLE_RATE) / i64::from(fps.max(1))
}

/// The record trace (`NBE_RECORD_TRACE=<file>`, a measurement knob, off by
/// default): one line per frame the loop handed off, naming the master frame
/// the View drew, the path, whether the draw's completion was awaited, whether
/// the encoder took the frame, and the encoder's submit count after it. The
/// record path's PTS is that count (`frame_index / fps`), so every frame in the
/// recording joins to the master frame it was drawn at. R31's probe reads it.
struct RecordTrace {
    out: std::io::BufWriter<std::fs::File>,
}

impl RecordTrace {
    fn from_env() -> Option<Self> {
        let path = std::env::var_os("NBE_RECORD_TRACE")?;
        match std::fs::File::create(&path) {
            Ok(f) => Some(Self {
                out: std::io::BufWriter::new(f),
            }),
            Err(e) => {
                tracing::warn!(err = %e, "NBE_RECORD_TRACE: cannot create the trace file; no trace");
                None
            }
        }
    }

    fn line(
        &mut self,
        kind: &str,
        frame: Option<u64>,
        waited: &str,
        submitted: bool,
        submits: u64,
    ) {
        use std::io::Write;
        let frame = frame.map(|f| f.to_string()).unwrap_or_else(|| "-".into());
        let _ = writeln!(
            self.out,
            "{kind} frame={frame} waited={waited} submitted={submitted} submits={submits}"
        );
    }

    fn flush(&mut self) {
        use std::io::Write;
        let _ = self.out.flush();
    }
}

/// Run the take. The third value is whether a `Stop` or `Abandon` ended it.
fn drive(args: ThreadArgs) -> (SessionResult, Sender<SessionResult>, bool) {
    let ThreadArgs {
        params,
        initial_sets,
        tap,
        frame_rx,
        control_rx,
        done_tx,
        skipped,
        ended_on_its_own: _,
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
        trace: RecordTrace::from_env(),
        base_frame: None,
        first_frame: None,
        last_frame: None,
        audio: None,
        asked_to_end: false,
    };
    // Eager open, off the loop: a missing encoder is known before the first
    // frame rather than discovered mid-take. The forced-unavailable seam
    // behaves exactly like missing hardware without an open attempt.
    if force_no_encoder() {
        d.setup_err = Some("hardware encoder forced unavailable (test seam)".into());
    } else {
        let p = &d.params;
        match EncodeSession::open(p.width, p.height, p.fps, RECORD_BITRATE) {
            Ok(mut enc) => {
                // A take consumes its units as they arrive: the session must
                // not keep the whole compressed take in memory.
                enc.set_drain_delivered(true);
                d.encoder = Some(enc);
            }
            Err(e) => d.setup_err = Some(e.to_string()),
        }
    }
    let result = d.run();
    (result, done_tx, d.asked_to_end)
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
                Ok(ControlMsg::Stop) => {
                    self.asked_to_end = true;
                    return self.finish_take();
                }
                Ok(ControlMsg::Abandon) => {
                    self.asked_to_end = true;
                    return self.abandon_take();
                }
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
            RecordMsg::Frame { rgba, frame } => {
                if self.encoder.is_none() {
                    // No encoder (failed open): shed the frame, count it, keep
                    // the take alive for `Unit`s / a loud stop.
                    self.count_skip(SkipPath::NoEncoder);
                    return Ok(());
                }
                // The frame's place on the take's master-clock timeline, when
                // the loop gave it one (a seam may not).
                let pts = match frame.map(|f| self.pts_index(f)) {
                    Some(None) => {
                        self.count_skip(SkipPath::BeforeFirst);
                        return Ok(());
                    }
                    Some(Some(index)) => Some(index),
                    None => None,
                };
                let enc = self.encoder.as_mut().expect("checked above");
                let encoded = match pts {
                    Some(index) => enc.encode_rgba_at(&rgba, index),
                    None => enc.encode_rgba(&rgba),
                };
                if let Some(t) = self.trace.as_mut() {
                    t.line("frame", frame, "-", encoded.is_ok(), enc.submitted_frames());
                }
                match encoded {
                    Ok(units) => {
                        if let Some(f) = frame {
                            self.note_encoded(f);
                        }
                        self.capture_sets();
                        for u in &units {
                            self.push_video(u)?;
                        }
                    }
                    Err(_) => {
                        // Hardware encode is all-or-nothing by design (no CPU
                        // fallback): a refusal degrades the frame, never the
                        // take. The encoder stays open; the next frame retries.
                        self.count_skip(SkipPath::Refused);
                    }
                }
            }
            RecordMsg::Surface {
                surface,
                drawn,
                frame,
            } => {
                if self.encoder.is_none() {
                    self.count_skip(SkipPath::NoEncoder);
                    return Ok(());
                }
                let Some(pts_index) = self.pts_index(frame) else {
                    self.count_skip(SkipPath::BeforeFirst);
                    return Ok(());
                };
                let enc = self.encoder.as_mut().expect("checked above");
                // The ONE difference from `Frame` above: the encoder reads the
                // allocation the compositor wrote, with no pool fetch and no
                // row-by-row swizzle. Everything after — parameter-set capture,
                // the push, the shed-on-refusal policy — is identical, because
                // the path a frame took must not change what the take is.
                //
                // R31: and it reads it only once the GPU has finished drawing
                // into it. A draw the GPU had not finished recorded the picture
                // the surface held before. A draw that does not finish within
                // DRAW_WAIT is a skip, never an encode.
                let encoded = crate::record::encode_after_draw(&drawn, || {
                    enc.encode_pixel_buffer_at(surface.pixel_buffer(), pts_index)
                });
                if let Some(t) = self.trace.as_mut() {
                    let waited = if encoded.is_some() { "ok" } else { "timeout" };
                    let submitted = matches!(encoded, Some(Ok(_)));
                    t.line(
                        "surface",
                        Some(frame),
                        waited,
                        submitted,
                        enc.submitted_frames(),
                    );
                }
                let Some(encoded) = encoded else {
                    tracing::warn!(
                        frame,
                        "record: the View's draw did not finish within DRAW_WAIT; frame skipped"
                    );
                    self.count_skip(SkipPath::DrawWait);
                    return Ok(());
                };
                match encoded {
                    Ok(units) => {
                        self.note_encoded(frame);
                        self.capture_sets();
                        for u in &units {
                            self.push_video(u)?;
                        }
                    }
                    Err(_) => {
                        self.count_skip(SkipPath::Refused);
                    }
                }
                // `surface` goes out of scope here, and THAT is the pool's
                // free signal — the loan's return. Written as a note rather
                // than an explicit `drop()`, because an explicit drop on the
                // last use is a no-op and a no-op that reads as load-bearing is
                // worse than nothing (§2a rule 4). What matters is the
                // negative: never store this `Arc` anywhere that outlives the
                // encode, or the pool starves by exactly one surface and the
                // symptom is a rising skip count with no other sign.
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
    ///
    /// When the tap and the video are both on the master clock, the audio is
    /// cut to the video: nothing before the take's first frame, and nothing
    /// past the end of the latest frame encoded (the rest waits for the video
    /// to reach it, or for the stop).
    fn drain_tap(&mut self) -> Result<(), RecordError> {
        if self.writer.is_none() {
            return Ok(());
        }
        let fps = self.params.fps;
        let segments = self.tap.drain_segments();
        let w = self.writer.as_mut().expect("checked above");
        // Each segment is placed where the driver pushed it (`AudioTap`'s
        // marks): a repeat is an overlap the cut drops, a jump a gap it pads.
        // Samples with no place (a take not on the master clock) go to the
        // writer as they came, as before.
        let Some(first) = self.first_frame else {
            for (_, samples) in &segments {
                if !samples.is_empty() {
                    w.push_audio(samples)?;
                }
            }
            return Ok(());
        };
        let audio = self
            .audio
            .get_or_insert_with(|| TakeAudio::new(audio_frame(first, fps)));
        for (pos, samples) in &segments {
            match pos {
                Some(pos) => audio.accept(*pos, samples),
                None if !samples.is_empty() => w.push_audio(samples)?,
                None => {}
            }
        }
        if let Some(last) = self.last_frame {
            let out = audio.take_until(audio_frame(last + 1, fps));
            if !out.is_empty() {
                w.push_audio(&out)?;
            }
        }
        Ok(())
    }

    /// The take's audio at its stop: wait (bounded) for the driver to push
    /// the take's last frame, then hand the writer exactly the video's span,
    /// padded where the tap fell short. A take not on the master clock keeps
    /// everything the tap held, as before.
    fn finish_audio(&mut self) -> Result<(), SessionError> {
        if self.writer.is_none() {
            return Ok(());
        }
        let fps = self.params.fps;
        let positioned = self.tap.end_position().is_some();
        let (Some(first), Some(last), true) = (self.first_frame, self.last_frame, positioned)
        else {
            let drained = self.tap.drain();
            if !drained.is_empty() {
                self.writer
                    .as_mut()
                    .expect("checked above")
                    .push_audio(&drained)
                    .map_err(SessionError::Record)?;
            }
            return Ok(());
        };
        let end = audio_frame(last + 1, fps);
        let waiting = Instant::now();
        while self.tap.end_position().is_some_and(|p| p < end)
            && waiting.elapsed() < AUDIO_TAIL_WAIT
        {
            std::thread::sleep(Duration::from_millis(2));
        }
        let waited = waiting.elapsed();
        let segments = self.tap.drain_segments();
        let mut audio = self
            .audio
            .take()
            .unwrap_or_else(|| TakeAudio::new(audio_frame(first, fps)));
        for (pos, samples) in &segments {
            if let Some(pos) = pos {
                audio.accept(*pos, samples);
            }
        }
        let (out, stats) = audio.finish(end);
        if !out.is_empty() {
            self.writer
                .as_mut()
                .expect("checked above")
                .push_audio(&out)
                .map_err(SessionError::Record)?;
        }
        let ms = |frames: u64| frames as f64 * 1000.0 / f64::from(crate::audio::SAMPLE_RATE);
        tracing::info!(
            first_frame = first,
            last_frame = last,
            trimmed_head_ms = ms(stats.trimmed_head),
            padded_ms = ms(stats.padded),
            padded_tail_ms = ms(stats.padded_tail),
            trimmed_tail_ms = ms(stats.trimmed_tail),
            overlapped_frames = stats.overlapped,
            tail_wait_ms = waited.as_secs_f64() * 1000.0,
            tap_discontinuities = self.tap.discontinuities(),
            tap_dropped_samples = self.tap.dropped(),
            "record: the take's audio is cut to its video's span"
        );
        Ok(())
    }

    /// `frame`'s PTS index on the take's timeline: its distance from the
    /// take's base frame (the first frame handed off). `None` for a frame
    /// before the base (it cannot be placed).
    fn pts_index(&mut self, frame: u64) -> Option<u64> {
        let base = *self.base_frame.get_or_insert(frame);
        frame.checked_sub(base)
    }

    /// The encoder took master frame `frame`: the take's span grows to it.
    fn note_encoded(&mut self, frame: u64) {
        self.first_frame.get_or_insert(frame);
        self.last_frame = Some(frame);
    }

    fn count_skip(&self, path: SkipPath) {
        self.skipped.count(path, 1);
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
        if let Some(t) = self.trace.as_mut() {
            t.flush();
        }
        // The frames the encoder still holds at stop: complete them and write
        // them, so every frame handed off before the stop is in the file.
        // A failed flush loses only that tail; the take still finishes.
        if let Some(enc) = self.encoder.as_mut() {
            match enc.flush() {
                Ok(tail) => {
                    if !tail.is_empty() {
                        self.capture_sets();
                    }
                    for u in &tail {
                        self.push_video(u)?;
                    }
                }
                Err(e) => {
                    tracing::warn!(err = %e, "record: the encoder's flush at stop failed; the take ends without its last frames");
                }
            }
        }
        // The take's audio, cut to the video's span, before finalizing.
        self.finish_audio()?;
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
        if let Some(t) = self.trace.as_mut() {
            t.flush();
        }
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
