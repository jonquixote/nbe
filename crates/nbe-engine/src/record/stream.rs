//! Stream session + stream thread (Prompt 10 WU4/WU5, SPEC §9.4, §16.14).
//!
//! Lifecycle owner between the directive path, the render loop and the
//! publisher: `stream.start` opens a [`StreamSession`], `stream.stop` /
//! `show.stop` quiescence closes it BEFORE `apply()` emits the ack (SPEC
//! §5.9.5: the ack is honest only after the effect is real) — the
//! `record.stop` `stop_and_finish` shape, mirrored.
//!
//! ## Shape (PR #30 repair round: the record thread's shape, for the stream)
//!
//! ```text
//! loop:    draw → hand_off_stream_surface (try_send Arc, never waits) ──╮
//!                                                                      ▼
//! thread:  Surface → encode_pixel_buffer_at (VideoToolbox, zero-copy) → FLV ─┐
//!          tap.drain() → AacEncoder → FLV ───────────────────────────────────┤
//!                                                                            ▼
//! task:                                          PublisherHandle (RTMP, tokio)
//! ```
//!
//! The render loop's only stream work is one bounded `try_send` of an `Arc`:
//! a full channel is a stream drop (`skipped_stream_frames`), never a wait.
//! PR #30's first version opened the encoder and encoded every frame inline
//! on the render loop — measured on the reference machine at 39.3 ms mean
//! per open (10 of 10 over the 33.3 ms budget, on every `stream.start`) and
//! 3.6 ms mean per frame — in a codebase whose record path already encoded on
//! its own thread.
//!
//! The stream thread owns the `!Send` handles (the VideoToolbox session and
//! the AudioToolbox AAC converter) as thread-locals, opens them eagerly off
//! both the loop and the directive path, and publishes through the
//! transport's bounded channel.
//!
//! ## Timeline
//!
//! Every media timestamp is media time. Video: the frame's position on the
//! show clock (the master frame the loop drew, relative to the stream's first
//! frame) becomes the encoder's PTS
//! ([`nbe_decode::encode::EncodeSession::encode_pixel_buffer_at`]), and the
//! RTMP timestamp is that PTS in ms — a shed frame leaves a gap rather than
//! pulling the video timeline behind the audio. Audio: retained AAC packets ×
//! 1024 samples at 48 kHz, after the same priming trim the record writer
//! applies. Both timelines start at the stream's first frame / first drained
//! sample, which the audio driver attaches within one audio cycle of
//! `stream.start` — alignment is within a frame, stated rather than measured
//! finer.
//!
//! ## Audio
//!
//! The stream has its own [`AudioTap`] (the record discipline: SPSC ring,
//! lock-free push on the audio thread, drained here). `stream.start`
//! publishes it in `state.stream_tap`; the audio driver attaches it beside
//! the record tap; every stop path clears it.
//!
//! ## Defined behavior
//!
//! * [`set_force_no_chain`] forces the chain-less path exactly as a machine
//!   with no zero-copy chain behaves — the mirror of record's
//!   `set_force_no_encoder`.
//! * [`set_force_close_error`] injects a teardown failure so the
//!   stop-withholds-ack path is testable without a real transport.
//! * A second `stream.start` while `Live` is refused upstream with
//!   `E_FORBIDDEN_STATE` and preserves the live session (§9.1).
//! * An encoder that fails to open on the thread (or the forced-unavailable
//!   seam) leaves the stream audio-only, every video frame counted in
//!   `skipped_stream_frames`; an AAC converter that fails leaves it
//!   video-only. Both are logged loudly; neither ends the stream (the start
//!   probe already answered; a thread-side failure is a degraded stream, not
//!   a refused one).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_decode::encode::{EncodeSession, EncodedUnit};
use nbe_decode::zerocopy::{SharedSurface, SurfacePool};

use super::rtmp::{
    parse_rtmp_url, spawn_publisher_with_envelope, PublisherHandle, PublisherState,
    AUDIO_BITRATE_BPS, VIDEO_BITRATE_BPS,
};
use crate::record::tap_path::Selection;
use crate::record::AudioTap;

/// Frames in flight between the loop and the stream thread (the record
/// `RECORD_CHANNEL_BOUND` shape). Beyond it the handoff sheds and counts a
/// stream drop; a deep queue would be stale airtime.
pub const STREAM_CHANNEL_BOUND: usize = 2;

/// Bounded wait for the stream thread to exit inside `stop_and_close`. The
/// thread drops its encoder on the way out (VideoToolbox invalidation), so
/// this is short-but-not-instant; with the transport's 800 ms it stays inside
/// the §16.1 2 s window beside record's parallel 1.5 s.
pub const STREAM_THREAD_STOP_TIMEOUT: Duration = Duration::from_millis(500);

/// The thread's wake quantum when no video arrives: audio drains at least
/// this often (one AAC packet is 21.3 ms).
const AUDIO_POLL: Duration = Duration::from_millis(10);

/// AAC-LC encoder priming trimmed at the head of the stream, as the record
/// writer trims it (`writer::AAC_PRIMING_TRIM_PACKETS`): the first packets
/// are codec delay, not content.
const AAC_PRIMING_TRIM_PACKETS: u64 = crate::record::writer::AAC_PRIMING_TRIM_PACKETS;

/// Forced-unavailable seam (tests only): when set, [`probe_stream_pool`]
/// reports no chain without touching the GPU.
static FORCE_NO_CHAIN: AtomicBool = AtomicBool::new(false);

/// Force (or release) the chain-less path. Test seam only.
pub fn set_force_no_chain(force: bool) {
    FORCE_NO_CHAIN.store(force, Ordering::SeqCst);
}

/// The seam state: true while forced chain-less.
pub fn force_no_chain() -> bool {
    FORCE_NO_CHAIN.load(Ordering::SeqCst)
}

/// Forced-teardown-failure seam (tests only): when set, [`StreamSession`]'s
/// close reports [`StreamError::Teardown`] instead of closing.
static FORCE_CLOSE_ERROR: AtomicBool = AtomicBool::new(false);

/// Force (or release) the teardown-failure path. Test seam only.
pub fn set_force_close_error(force: bool) {
    FORCE_CLOSE_ERROR.store(force, Ordering::SeqCst);
}

/// Build the stream's own surface pool, or `None` when this machine has no
/// lawful streaming chain (SPEC §0.1 assumption 24 as rescoped by v0.4.2:
/// recording alone holds the readback allowance, so no chain = no path).
///
/// The probe IS the pool: `stream.start` keeps what it built (the record
/// `record.start` shape) instead of building one to answer a yes/no, dropping
/// it, and letting the render loop build another on its first live frame —
/// which is what PR #30 first did, putting a VRAM allocation on the loop.
/// A `None` device (headless, or the loop has not run) is no chain.
pub fn probe_stream_pool(device: &Option<Arc<wgpu::Device>>) -> Option<SurfacePool> {
    if FORCE_NO_CHAIN.load(Ordering::SeqCst) {
        return None;
    }
    crate::record::stream_zerocopy_pool(device.as_ref()?).ok()
}

/// Yes/no form of [`probe_stream_pool`] (tests and diagnostics).
pub fn chain_available(device: &Option<Arc<wgpu::Device>>) -> bool {
    probe_stream_pool(device).is_some()
}

/// Opening or closing a stream fails loudly, with stable tokens.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// No zero-copy chain (seam-forced or genuinely absent): `E_NO_ZEROCOPY`.
    #[error("E_NO_ZEROCOPY: {0}")]
    NoChain(String),
    /// Teardown did not confirm (thread or transport): `E_NETWORK`.
    #[error("E_NETWORK: {0}")]
    Teardown(String),
}

/// What the stream encodes at: geometry, rate, and the §9.4 envelope as the
/// manifest sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamParams {
    pub width: u32,
    pub height: u32,
    /// The show's frame rate (`show.video.frameRate`, the house rate): the
    /// encoder's timebase and keyframe interval, and the rate the stream's
    /// timestamps advance at.
    pub fps: u32,
    pub video_bitrate_bps: u32,
    pub audio_bitrate_bps: u32,
}

impl StreamParams {
    /// §9.4 defaults at this geometry and rate: 8 Mbps video (inside the
    /// 6–12 Mbps recommendation), 192 kbps AAC.
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            width,
            height,
            fps: fps.max(1),
            video_bitrate_bps: VIDEO_BITRATE_BPS as u32,
            audio_bitrate_bps: AUDIO_BITRATE_BPS as u32,
        }
    }

    /// Apply the manifest's `outputs.stream.videoBitrateKbps` /
    /// `audioBitrateKbps` where present (the schema bounds them).
    pub fn with_output(mut self, output: Option<&nbe_core::manifest::StreamOutput>) -> Self {
        if let Some(o) = output {
            if let Some(kbps) = o.video_bitrate_kbps {
                self.video_bitrate_bps = kbps.saturating_mul(1000);
            }
            if let Some(kbps) = o.audio_bitrate_kbps {
                self.audio_bitrate_bps = kbps.saturating_mul(1000);
            }
        }
        self
    }

    /// Video + audio bits per second: what `streamBufferMs` divides by.
    pub fn envelope_bps(&self) -> u64 {
        self.video_bitrate_bps as u64 + self.audio_bitrate_bps as u64
    }
}

/// One unit of stream work from the loop.
#[derive(Debug)]
pub enum StreamMsg {
    /// A surface the compositor drew, loaned without a copy, with the master
    /// frame it was drawn for (the stream's video timeline).
    Surface {
        surface: Arc<SharedSurface>,
        frame: u64,
    },
}

#[derive(Debug)]
enum StreamControl {
    Stop,
}

/// What the stream thread has done — observable to tests and measurements,
/// never on the wire.
#[derive(Debug, Default)]
pub struct StreamStats {
    /// The encoder opened on the thread (eagerly, at spawn).
    pub encoder_ready: AtomicBool,
    /// Time the encoder open took on the thread, µs (off the loop and off
    /// the directive path by construction).
    pub encoder_open_us: AtomicU64,
    /// The AAC converter opened and the audio sequence header was queued.
    pub aac_ready: AtomicBool,
    /// The bitrate the AAC converter reports it is using (read back from
    /// AudioToolbox, not echoed from the request): proof the manifest's
    /// `audioBitrateKbps` reached the codec.
    pub aac_bit_rate: AtomicU64,
    pub video_frames_encoded: AtomicU64,
    /// Total encode-call time on the thread, µs.
    pub encode_us_total: AtomicU64,
    pub video_units_published: AtomicU64,
    pub audio_packets_published: AtomicU64,
}

/// A live stream: the publish target, the selection, the publisher, and the
/// stream thread's endpoints. Deliberately `Send` so engine state can hold
/// it; the `!Send` half lives on the stream thread.
pub struct StreamSession {
    endpoint: String,
    selection: Selection,
    params: StreamParams,
    closed: bool,
    /// The RTMP publisher: spawned at open (background dial, never blocking
    /// the directive path). Shared with the stream thread, which feeds it.
    /// `None` when there is no runtime (sync unit tests) or the endpoint is
    /// not an RTMP publish target.
    publisher: Option<Arc<PublisherHandle>>,
    tap: Arc<AudioTap>,
    frame_tx: Option<SyncSender<StreamMsg>>,
    control_tx: Option<Sender<StreamControl>>,
    done_rx: Option<Receiver<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// The stream's own surfaces (stream-only frames). Lives and dies with
    /// the session, like the record take's pool — so no teardown path can
    /// forget it.
    surface_pool: Option<Arc<SurfacePool>>,
    stats: Arc<StreamStats>,
}

impl StreamSession {
    /// Open a session on `endpoint`: spawns the publisher (it dials in the
    /// background) and, when there is a publisher to feed, the stream thread
    /// (which opens its encoders on itself). Nothing here waits.
    pub fn open(
        endpoint: impl Into<String>,
        selection: Selection,
        params: StreamParams,
        skipped: Arc<AtomicU64>,
    ) -> Self {
        let endpoint = endpoint.into();
        let publisher = maybe_spawn_publisher(&endpoint, params.envelope_bps()).map(Arc::new);
        let tap = Arc::new(AudioTap::new());
        let stats = Arc::new(StreamStats::default());
        let mut session = Self {
            endpoint,
            selection,
            params,
            closed: false,
            publisher,
            tap,
            frame_tx: None,
            control_tx: None,
            done_rx: None,
            handle: None,
            surface_pool: None,
            stats,
        };
        if let Some(publisher) = session.publisher.clone() {
            let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel(STREAM_CHANNEL_BOUND);
            let (control_tx, control_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let args = StreamThreadArgs {
                params,
                publisher,
                tap: session.tap.clone(),
                frame_rx,
                control_rx,
                done_tx,
                skipped,
                stats: session.stats.clone(),
            };
            let handle = std::thread::Builder::new()
                .name("nbe-stream".into())
                .spawn(move || run_stream_thread(args))
                .expect("the stream thread must start");
            session.frame_tx = Some(frame_tx);
            session.control_tx = Some(control_tx);
            session.done_rx = Some(done_rx);
            session.handle = Some(handle);
        }
        session
    }

    /// The publish target this session was opened on.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The frame-path selection probed at start.
    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// What the stream encodes at.
    pub fn params(&self) -> StreamParams {
        self.params
    }

    /// True once the engine closed the session.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// True while the session owns a live publisher.
    pub fn has_publisher(&self) -> bool {
        self.publisher.is_some()
    }

    /// The transport (tests drive it directly with synthetic payloads).
    pub fn publisher(&self) -> Option<&PublisherHandle> {
        self.publisher.as_deref()
    }

    /// The stream's audio tap (published to the audio driver at start).
    pub fn tap(&self) -> Arc<AudioTap> {
        self.tap.clone()
    }

    /// The stream thread's counters.
    pub fn stats(&self) -> Arc<StreamStats> {
        self.stats.clone()
    }

    /// The loop's handoff endpoint (`try_send` only), or `None` when there is
    /// no stream thread to feed.
    pub fn frame_sender(&self) -> Option<SyncSender<StreamMsg>> {
        self.frame_tx.clone()
    }

    /// The stream's own surface pool, when it runs zero-copy.
    pub fn surface_pool(&self) -> Option<Arc<SurfacePool>> {
        self.surface_pool.clone()
    }

    /// Attach the pool `stream.start` probed with.
    pub fn set_surface_pool(&mut self, pool: Arc<SurfacePool>) {
        self.surface_pool = Some(pool);
    }

    /// Bytes the transport currently holds (accepted, unwritten).
    pub fn buffered_bytes(&self) -> usize {
        self.publisher
            .as_ref()
            .map(|p| p.buffered_bytes())
            .unwrap_or(0)
    }

    /// `streamBufferMs`, honest: the transport's buffered bytes through this
    /// stream's envelope bitrate; 0.0 with no transport.
    pub fn stream_buffer_ms(&self) -> f64 {
        self.publisher
            .as_ref()
            .map(|p| p.buffer_ms())
            .unwrap_or(0.0)
    }

    /// Transport liveness (`Live` / `Reconnecting` / `Closed`). The engine's
    /// `StreamState` stays `Live` while this reads `Reconnecting` — that
    /// split IS the survival guarantee (§9.5).
    pub fn publisher_state(&self) -> PublisherState {
        self.publisher
            .as_ref()
            .map(|p| p.publisher_state())
            .unwrap_or(PublisherState::Closed)
    }

    /// Close gracefully: the stream thread has exited and the transport is
    /// gone BEFORE this returns, so the ack that follows is honest. Both
    /// waits are bounded and async (tokio sleep, never blocking the
    /// executor); either one expiring is a withheld ack.
    ///
    /// With the close-error seam armed this reports [`StreamError::Teardown`]
    /// and closes nothing (the caller still ends `Live`; dropping the session
    /// then abandons it).
    pub async fn stop_and_close(&mut self) -> Result<(), StreamError> {
        if FORCE_CLOSE_ERROR.load(Ordering::SeqCst) {
            return Err(StreamError::Teardown(
                "stream teardown failed (injected): transport did not confirm shutdown".into(),
            ));
        }
        // Thread first, so nothing is published into a closing transport.
        let thread_exited = self.stop_thread().await;
        let transport_closed = match self.publisher.take() {
            Some(p) => p.shutdown_and_wait(PublisherHandle::stop_timeout()).await,
            None => true,
        };
        self.closed = true;
        self.surface_pool = None;
        if !thread_exited {
            return Err(StreamError::Teardown(format!(
                "stream thread did not exit within {} ms",
                STREAM_THREAD_STOP_TIMEOUT.as_millis()
            )));
        }
        if !transport_closed {
            return Err(StreamError::Teardown(
                "stream teardown timed out: transport did not confirm shutdown".into(),
            ));
        }
        Ok(())
    }

    /// Drop the session WITHOUT a graceful close (`force=true`): signal the
    /// thread and the transport, wait for neither.
    pub fn abandon(&mut self) {
        self.frame_tx.take();
        if let Some(c) = self.control_tx.take() {
            let _ = c.send(StreamControl::Stop);
        }
        self.done_rx.take();
        // Dropping the handle detaches: the thread exits on the signal.
        self.handle.take();
        if let Some(p) = self.publisher.take() {
            p.shutdown_signal();
        }
        self.surface_pool = None;
        self.closed = true;
    }

    async fn stop_thread(&mut self) -> bool {
        self.frame_tx.take();
        if let Some(c) = self.control_tx.take() {
            let _ = c.send(StreamControl::Stop);
        }
        let Some(rx) = self.done_rx.take() else {
            return true;
        };
        let start = Instant::now();
        loop {
            match rx.try_recv() {
                // Reported, or gone without reporting (a panicked thread has
                // exited too): either way it is no longer running.
                Ok(()) | Err(TryRecvError::Disconnected) => {
                    if let Some(h) = self.handle.take() {
                        let _ = h.join();
                    }
                    return true;
                }
                Err(TryRecvError::Empty) => {}
            }
            if start.elapsed() >= STREAM_THREAD_STOP_TIMEOUT {
                self.handle.take();
                return false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

impl Drop for StreamSession {
    /// A session dropped without a close (a failed teardown the caller still
    /// ends `Live` on, or a test) is abandoned: the thread and the transport
    /// are signalled rather than left running.
    fn drop(&mut self) {
        if !self.closed {
            self.abandon();
        }
    }
}

/// Spawn the publisher for `endpoint`, if one can exist here: a tokio runtime
/// must be running (the directive path always has one; sync unit tests do
/// not) and the endpoint must parse as an RTMP publish target.
fn maybe_spawn_publisher(endpoint: &str, envelope_bps: u64) -> Option<PublisherHandle> {
    if tokio::runtime::Handle::try_current().is_err() {
        return None;
    }
    let url = parse_rtmp_url(endpoint).ok()?;
    Some(spawn_publisher_with_envelope(url, envelope_bps))
}

/// The loop's whole stream cost: one bounded `try_send` of the drawn
/// surface's `Arc`. Never waits, never encodes.
///
/// A full channel is a stream drop — counted on `skipped_stream_frames`,
/// never a record skip or a View drop (G1); the `Arc` is dropped here, which
/// is the frame given up. A disconnected channel means the stream thread
/// already stopped (a `stream.stop` landed mid-tick): the stream is over, not
/// dropping frames, so nothing is counted. Returns whether the frame went.
pub fn hand_off_stream_surface(
    tx: &SyncSender<StreamMsg>,
    surface: Arc<SharedSurface>,
    frame: u64,
    skipped: &AtomicU64,
) -> bool {
    match tx.try_send(StreamMsg::Surface { surface, frame }) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            skipped.fetch_add(1, Ordering::SeqCst);
            false
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

// ---------------------------------------------------------------------------
// The stream thread.
// ---------------------------------------------------------------------------

struct StreamThreadArgs {
    params: StreamParams,
    publisher: Arc<PublisherHandle>,
    tap: Arc<AudioTap>,
    frame_rx: Receiver<StreamMsg>,
    control_rx: Receiver<StreamControl>,
    done_tx: Sender<()>,
    /// The engine-state `skipped_stream_frames`: thread-side drops (no
    /// encoder, refused encodes, publish sheds) land beside the loop's.
    skipped: Arc<AtomicU64>,
    stats: Arc<StreamStats>,
}

fn run_stream_thread(args: StreamThreadArgs) {
    {
        let mut t = StreamThread::open(&args);
        t.run(&args);
        // `t` drops here: the VideoToolbox session is invalidated, which
        // releases any buffer it still retains — before `done` says the
        // thread is finished with the pool's surfaces.
    }
    let _ = args.done_tx.send(());
}

struct StreamThread {
    encoder: Option<EncodeSession>,
    aac: Option<nbe_decode::aac::AacEncoder>,
    /// The master frame of the stream's first video frame: PTS 0.
    base_frame: Option<u64>,
    /// The AVC sequence header went out (once per stream; the transport
    /// replays its send-time cache after every redial).
    seq_sent: bool,
    priming_to_skip: u64,
    audio_packets: u64,
    /// An odd trailing sample held for the next drain, so stereo pairs never
    /// split across drains.
    carry: Option<f32>,
}

impl StreamThread {
    fn open(a: &StreamThreadArgs) -> Self {
        let p = &a.params;
        let encoder = if crate::record::session::force_no_encoder() {
            tracing::error!("stream thread: hardware encoder forced unavailable (test seam); video frames will be dropped");
            None
        } else {
            let started = Instant::now();
            match EncodeSession::open(p.width, p.height, p.fps, p.video_bitrate_bps) {
                Ok(enc) => {
                    a.stats
                        .encoder_open_us
                        .store(started.elapsed().as_micros() as u64, Ordering::SeqCst);
                    a.stats.encoder_ready.store(true, Ordering::SeqCst);
                    Some(enc)
                }
                Err(e) => {
                    tracing::error!(err = %e, "stream thread: encoder open failed; video frames will be dropped");
                    None
                }
            }
        };
        let aac = match nbe_decode::aac::AacEncoder::with_bitrate(p.audio_bitrate_bps) {
            Ok(enc) => match enc.bare_audio_specific_config() {
                Some(asc) => {
                    a.stats
                        .aac_bit_rate
                        .store(enc.encode_bit_rate().unwrap_or(0) as u64, Ordering::SeqCst);
                    // The codec's own ASC, at the head of the audio timeline.
                    let mut seq = vec![0xAF, 0x00];
                    seq.extend_from_slice(&asc);
                    let _ = a.publisher.try_publish_audio(seq, 0);
                    a.stats.aac_ready.store(true, Ordering::SeqCst);
                    Some(enc)
                }
                None => {
                    tracing::error!("stream thread: AAC cookie holds no AudioSpecificConfig; stream is video-only");
                    None
                }
            },
            Err(e) => {
                tracing::error!(err = %e, "stream thread: AAC unavailable; stream is video-only");
                None
            }
        };
        Self {
            encoder,
            aac,
            base_frame: None,
            seq_sent: false,
            priming_to_skip: AAC_PRIMING_TRIM_PACKETS,
            audio_packets: 0,
            carry: None,
        }
    }

    fn run(&mut self, a: &StreamThreadArgs) {
        loop {
            match a.control_rx.try_recv() {
                Ok(StreamControl::Stop) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => {}
            }
            match a.frame_rx.recv_timeout(AUDIO_POLL) {
                Ok(StreamMsg::Surface { surface, frame }) => self.encode_video(a, surface, frame),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.drain_audio(a);
        }
    }

    fn drop_frame(a: &StreamThreadArgs, n: u64) {
        a.skipped.fetch_add(n, Ordering::SeqCst);
    }

    fn encode_video(&mut self, a: &StreamThreadArgs, surface: Arc<SharedSurface>, frame: u64) {
        let Some(enc) = self.encoder.as_mut() else {
            Self::drop_frame(a, 1);
            return;
        };
        let base = *self.base_frame.get_or_insert(frame);
        let Some(pts_index) = frame.checked_sub(base) else {
            Self::drop_frame(a, 1);
            return;
        };
        let started = Instant::now();
        let result = enc.encode_pixel_buffer_at(surface.pixel_buffer(), pts_index);
        a.stats
            .encode_us_total
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::SeqCst);
        // Our hold ends here, before publishing. VideoToolbox may still hold
        // the buffer; the pool's free rule waits for that release, so this
        // drop is the Rust half of the loan's return, not the whole of it.
        drop(surface);
        let units = match result {
            Ok(units) => units,
            Err(e) => {
                tracing::debug!(err = %e, frame, "stream thread: encode refused; frame dropped");
                Self::drop_frame(a, 1);
                return;
            }
        };
        a.stats.video_frames_encoded.fetch_add(1, Ordering::SeqCst);
        if units.is_empty() {
            return;
        }
        if !self.seq_sent {
            // The first unit is the forced IDR, and the parameter sets are
            // captured from its format description — so they exist now. A
            // stream must never carry NALUs its peer has no avcC for.
            let seq = enc
                .parameter_sets()
                .and_then(|(sps, pps)| avc_sequence_header(&sps, &pps));
            let Some(seq) = seq else {
                Self::drop_frame(a, units.len() as u64);
                return;
            };
            // Cached at send time even if shed, so a redial re-announces it.
            if !a
                .publisher
                .try_publish_video(seq, media_ts_ms(units[0].pts_seconds))
            {
                Self::drop_frame(a, 1);
            }
            self.seq_sent = true;
        }
        for u in &units {
            if a.publisher
                .try_publish_video(flv_video_tag(u), media_ts_ms(u.pts_seconds))
            {
                a.stats.video_units_published.fetch_add(1, Ordering::SeqCst);
            } else {
                Self::drop_frame(a, 1);
            }
        }
    }

    fn drain_audio(&mut self, a: &StreamThreadArgs) {
        let drained = a.tap.drain();
        if drained.is_empty() {
            return;
        }
        let Some(aac) = self.aac.as_mut() else {
            return;
        };
        let mut pcm = Vec::with_capacity(drained.len() + 1);
        pcm.extend(self.carry.take());
        pcm.extend_from_slice(&drained);
        if pcm.len() % 2 == 1 {
            self.carry = pcm.pop();
        }
        match aac.encode_interleaved_f32(&pcm) {
            Ok(packets) => {
                for f in packets {
                    if self.priming_to_skip > 0 {
                        self.priming_to_skip -= 1;
                        continue;
                    }
                    let ts = audio_ts_ms(self.audio_packets);
                    self.audio_packets += 1;
                    let mut tag = Vec::with_capacity(2 + f.data.len());
                    tag.extend_from_slice(&[0xAF, 0x01]);
                    tag.extend_from_slice(&f.data);
                    if a.publisher.try_publish_audio(tag, ts) {
                        a.stats
                            .audio_packets_published
                            .fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            Err(e) => {
                tracing::error!(err = %e, "stream thread: AAC encode failed; stream continues video-only");
                self.aac = None;
            }
        }
    }
}

/// A video unit's RTMP timestamp: its PTS in whole milliseconds.
pub fn media_ts_ms(pts_seconds: f64) -> u32 {
    (pts_seconds * 1000.0).round().max(0.0) as u32
}

/// The `n`th retained AAC packet's RTMP timestamp: `n × 1024` samples at
/// 48 kHz, in whole milliseconds (exact in integer arithmetic).
pub fn audio_ts_ms(n: u64) -> u32 {
    (n * nbe_decode::aac::FRAMES_PER_PACKET as u64 * 1000
        / nbe_decode::aac::INPUT_SAMPLE_RATE as u64) as u32
}

/// Convert one encoder access unit (AVCC length-prefixed NALs) to an FLV
/// video tag payload: `[frame|codec, avc-type, cts×3] + NALs`. Keyframes ride
/// `0x17`, inter frames `0x27`; `avc-type` is always `0x01` (NALU — sequence
/// headers ride separately). CTS is zero because frame reordering is off
/// (`AllowFrameReordering = false`): decode order is presentation order, so
/// the RTMP timestamp (the PTS) is also the DTS.
pub fn flv_video_tag(unit: &EncodedUnit) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + unit.data.len());
    out.push(if unit.is_keyframe { 0x17 } else { 0x27 });
    out.push(0x01);
    out.extend_from_slice(&[0x00, 0x00, 0x00]);
    out.extend_from_slice(&unit.data);
    out
}

/// Build an FLV AVC sequence header (`0x17 0x00` + avcC) from REAL parameter
/// sets (the encoder's own SPS/PPS). Profile/compat/level are copied from the
/// SPS (`sps[1..4]`), never hardcoded. Refuses empty or mistyped sets loudly
/// (`None`): a sequence header with no parameters is worse than none.
pub fn avc_sequence_header(sps: &[u8], pps: &[u8]) -> Option<Vec<u8>> {
    if sps.len() < 4 || pps.is_empty() {
        return None;
    }
    // NAL validation by type, not by exact header byte: real VideoToolbox
    // SPS NALs arrive with nal_ref_idc != 3 (0x27 observed on hardware).
    if sps[0] & 0x80 != 0 || sps[0] & 0x1F != 7 {
        return None;
    }
    if pps[0] & 0x80 != 0 || pps[0] & 0x1F != 8 {
        return None;
    }
    let mut out = vec![
        0x17, 0x00, 0x00, 0x00, 0x00, // FLV video tag header: keyframe, AVC, sequence
        0x01, sps[1], sps[2], sps[3], // avcC version 1 + profile/compat/level from the SPS
        0xFF, 0xE1, // 4-byte NALU lengths, one SPS
    ];
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(0x01); // one PPS
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> StreamParams {
        StreamParams::new(640, 360, 30)
    }

    #[test]
    fn force_seam_reports_chain_less_without_touching_hardware() {
        set_force_no_chain(true);
        assert!(!chain_available(&None));
        set_force_no_chain(false);
    }

    #[tokio::test]
    async fn close_error_seam_fails_loudly_with_the_network_token() {
        let sel = crate::record::tap_path::select_stream(true).unwrap();
        let mut s = StreamSession::open(
            "rtmp://127.0.0.1:9/live/k",
            sel,
            params(),
            Arc::new(AtomicU64::new(0)),
        );
        set_force_close_error(true);
        let err = s.stop_and_close().await.expect_err("armed seam must fail");
        assert!(err.to_string().starts_with("E_NETWORK: "));
        assert!(!s.is_closed(), "a failed close closes nothing");
        set_force_close_error(false);
        s.stop_and_close().await.expect("released seam must close");
        assert!(s.is_closed());
    }

    #[test]
    fn a_transportless_session_has_no_thread_to_feed() {
        // No runtime → no publisher → no thread: the loop's handoff finds no
        // sender and hands nothing off.
        let sel = crate::record::tap_path::select_stream(true).unwrap();
        let s = StreamSession::open(
            "rtmp://127.0.0.1:9/live/k",
            sel,
            params(),
            Arc::new(AtomicU64::new(0)),
        );
        assert!(!s.has_publisher());
        assert!(s.frame_sender().is_none());
    }

    #[test]
    fn params_take_the_manifest_bitrates() {
        let out: nbe_core::manifest::StreamOutput = serde_json::from_value(serde_json::json!({
            "url": "rtmp://h/a/k", "videoBitrateKbps": 6000, "audioBitrateKbps": 128
        }))
        .unwrap();
        let p = StreamParams::new(1920, 1080, 60).with_output(Some(&out));
        assert_eq!(p.video_bitrate_bps, 6_000_000);
        assert_eq!(p.audio_bitrate_bps, 128_000);
        assert_eq!(p.fps, 60);
        assert_eq!(p.envelope_bps(), 6_128_000);
        let d = StreamParams::new(1920, 1080, 30).with_output(None);
        assert_eq!(d.envelope_bps(), VIDEO_BITRATE_BPS + AUDIO_BITRATE_BPS);
    }

    #[test]
    fn timestamps_are_media_time() {
        // Video: PTS in ms. At 60 fps successive frames are 16–17 ms apart,
        // at 30 fps 33–34 ms: the timeline advances at the show's rate.
        let at = |fps: u32, i: u64| media_ts_ms(i as f64 / fps as f64);
        assert_eq!(
            (0..4).map(|i| at(60, i)).collect::<Vec<_>>(),
            [0, 17, 33, 50]
        );
        assert_eq!(
            (0..4).map(|i| at(30, i)).collect::<Vec<_>>(),
            [0, 33, 67, 100]
        );
        assert_eq!(at(60, 60), 1000);
        // Audio: 1024 samples at 48 kHz = 21.333 ms per packet.
        assert_eq!(audio_ts_ms(0), 0);
        assert_eq!(audio_ts_ms(3), 64);
        assert_eq!(audio_ts_ms(375), 8000);
    }

    #[test]
    fn avc_sequence_header_refuses_empty_or_mistyped_sets() {
        assert!(avc_sequence_header(&[], &[]).is_none());
        assert!(avc_sequence_header(&[0x67, 0x64], &[0x68]).is_none());
        assert!(avc_sequence_header(&[0x65, 0x64, 0x00, 0x1F], &[0x68]).is_none());
        assert!(avc_sequence_header(&[0x67, 0x64, 0x00, 0x1F], &[0x67]).is_none());
        let sps = vec![0x67, 0x64, 0x00, 0x1F, 0xAA, 0xBB];
        let pps = vec![0x68, 0xCC];
        let seq = avc_sequence_header(&sps, &pps).expect("valid sets build");
        assert!(seq.starts_with(&[0x17, 0x00, 0x00, 0x00, 0x00]));
        assert_eq!(&seq[5..10], &[0x01, 0x64, 0x00, 0x1F, 0xFF]);
        assert!(seq.windows(sps.len()).any(|w| w == sps.as_slice()));
        assert!(seq.windows(pps.len()).any(|w| w == pps.as_slice()));
    }

    #[test]
    fn avc_sequence_header_accepts_real_hardware_sps_shapes() {
        let sps = vec![0x27, 0x64, 0x00, 0x28, 0xAC, 0x13];
        let pps = vec![0x28, 0xEE, 0x1F, 0x2C];
        let seq = avc_sequence_header(&sps, &pps).expect("hardware sets build");
        assert_eq!(&seq[5..10], &[0x01, 0x64, 0x00, 0x28, 0xFF]);
    }

    #[test]
    fn flv_video_tag_marks_keyframes_and_never_sequence() {
        let key = EncodedUnit {
            data: vec![0x01, 0x02],
            is_keyframe: true,
            pts_seconds: 0.0,
        };
        let inter = EncodedUnit {
            data: vec![0x03],
            is_keyframe: false,
            pts_seconds: 1.0,
        };
        assert_eq!(&flv_video_tag(&key)[..5], &[0x17, 0x01, 0x00, 0x00, 0x00]);
        assert_eq!(&flv_video_tag(&inter)[..5], &[0x27, 0x01, 0x00, 0x00, 0x00]);
        assert_eq!(&flv_video_tag(&key)[5..], &[0x01, 0x02]);
    }
}
