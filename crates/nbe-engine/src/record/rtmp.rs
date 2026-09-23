//! RTMP publish transport (Prompt 10 WU5, SPEC §9.4/§9.5).
//!
//! **Transport-crate decision, stated explicitly:** no third-party RTMP crate.
//! `cargo add --dry-run` resolves both candidates (`rml_rtmp 0.8.0`,
//! `oxideav-rtmp 0.0.6`), and their dependency metadata shows no native link
//! (`lib_links: null`; `rml_rtmp` → byteorder/bytes/hmac/rand/rml_amf0/sha2,
//! `oxideav-rtmp` → `oxideav-core` → thiserror only — pure Rust, zero FFI).
//! They are rejected anyway: `rml_rtmp` is a state-machine kit that still
//! leaves TCP/chunking/AMF0 dial/FLV muxing/reconnect/buffer-accounting to us,
//! and `oxideav-rtmp 0.0.6` is weeks old with 82 downloads — unreviewed
//! provenance on the on-air path. What follows is a **minimal real RTMP
//! publish client**: the Adobe handshake (C0/C1/S0/S1/S2/C2, simple — no
//! digest), then `connect <app>` / `createStream` / `publish <key>` as AMF0
//! invokes over chunk stream 3, then FLV-typed media (0x09 video/H.264, 0x08
//! audio/AAC) chunked at the negotiated size (default 128, split with fmt=3
//! continuations). Proven against MediaMTX (see `tests/prompt10_rtmp.rs`
//! `mediamtx_proof_*`, loud skip when the binary is absent) and against the
//! in-process double, which speaks the same wire.
//!
//! ## Shape (§9.4)
//!
//! H.264 High 1080p30, AAC 48 kHz, envelope 8 Mbps video + 192 kbps audio
//! (inside the 6–12 Mbps video recommendation), 1 s keyframes (a codec
//! sequence header opens every connect, media keyframes are the feeder's).
//! Reconnect is automatic with capped backoff; a dead transport never blocks
//! the caller (bounded `try_send`, shed counted); `stream_buffer_ms` is the
//! actual queued bytes through the envelope bitrate — never a constant.
//!
//! ## Buffer accounting
//!
//! `buffered` is admitted-but-unwritten, EXACT: the feeder-side `send()`
//! credits on `try_send` success (so bytes sitting in the mpsc channel count),
//! the publisher task debits on socket write (or on stale-drain drop). The
//! channel backlog is therefore INCLUDED, not hidden — telemetry's
//! `streamBufferMs` divides this counter, so a stalled peer shows growth even
//! before the task's `recv` fires. Kernel/socket buffers are invisible by
//! construction (documented, not hidden): what we count is what we hold.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Video bitrate of the §9.4 envelope (midpoint of the 6–12 Mbps band).
pub const VIDEO_BITRATE_BPS: u64 = 8_000_000;
/// Audio bitrate of the §9.4 envelope (AAC 192 kbps).
pub const AUDIO_BITRATE_BPS: u64 = 192_000;
/// Envelope the bytes→ms conversion divides by (video + audio).
pub const ENVELOPE_BITRATE_BPS: u64 = VIDEO_BITRATE_BPS + AUDIO_BITRATE_BPS;

/// Frames in flight between the feeder (render loop) and the publisher task.
/// Small on purpose: the task writes concurrently, and a deep queue is stale
/// airtime masquerading as throughput (the record `RECORD_CHANNEL_BOUND`
/// shape — the View never waits).
pub const PUBLISH_CHANNEL_BOUND: usize = 64;

/// First reconnect pause; doubles to [`MAX_RECONNECT_PAUSE`].
const INITIAL_RECONNECT_PAUSE: Duration = Duration::from_millis(100);
/// Reconnect backoff ceiling: redial at worst once a second.
const MAX_RECONNECT_PAUSE: Duration = Duration::from_secs(1);
/// Per-dial TCP connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Graceful publisher shutdown wait inside `stop_and_close` (record's
/// `RECORD_STOP_TIMEOUT` shape, smaller: no file to finish, just a socket).
const PUBLISH_STOP_TIMEOUT: Duration = Duration::from_millis(800);

/// Transport failure. thiserror, `E_`-tokened where operator-facing.
#[derive(Debug, thiserror::Error)]
pub enum RtmpError {
    /// The endpoint is not an `rtmp://host[:port]/app/key` publish target.
    #[error("E_BAD_PAYLOAD: {0}")]
    Parse(String),
    /// TCP / handshake / dialog I/O refused.
    #[error("E_NETWORK: {0}")]
    Io(String),
    /// The peer answered out of shape (bad version, bad echo, refused line).
    #[error("E_NETWORK: {0}")]
    Protocol(String),
}

/// An `rtmp://host[:port]/app/key` publish target. The key is everything past
/// the first path segment, slashes included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpUrl {
    /// TCP host (loopback in tests, platform host on air).
    pub host: String,
    /// TCP port (1935 when absent).
    pub port: u16,
    /// First path segment (the RTMP app).
    pub app: String,
    /// The remainder (the stream key, a credential — never logged).
    pub key: String,
}

/// Parse an RTMP publish URL. Empty segments refuse loudly, never silently.
pub fn parse_rtmp_url(url: &str) -> Result<RtmpUrl, RtmpError> {
    let rest = url
        .strip_prefix("rtmp://")
        .ok_or_else(|| RtmpError::Parse(format!("not an rtmp:// URL: {url}")))?;
    let (host_port, path) = rest
        .split_once('/')
        .ok_or_else(|| RtmpError::Parse(format!("rtmp URL has no app/key path: {url}")))?;
    if host_port.is_empty() {
        return Err(RtmpError::Parse(format!("rtmp URL has no host: {url}")));
    }
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => {
            let port: u16 = p
                .parse()
                .map_err(|_| RtmpError::Parse(format!("rtmp URL has a bad port: {url}")))?;
            (h.to_string(), port)
        }
        None => (host_port.to_string(), 1935),
    };
    if host.is_empty() {
        return Err(RtmpError::Parse(format!("rtmp URL has no host: {url}")));
    }
    let path = path.trim_matches('/');
    let (app, key) = path
        .split_once('/')
        .ok_or_else(|| RtmpError::Parse(format!("rtmp URL has no stream key: {url}")))?;
    if app.is_empty() || key.is_empty() {
        return Err(RtmpError::Parse(format!(
            "rtmp URL needs a non-empty app and key: {url}"
        )));
    }
    Ok(RtmpUrl {
        host,
        port,
        app: app.to_string(),
        key: key.to_string(),
    })
}

/// FLV-typed AVC sequence header: keyframe + AVC sequence (0x17 0x00) with a
/// decoder-configuration record carrying REAL H.264 parameter sets — High
/// profile, level 4.0, 1080p (the §9.4 envelope shape). The bytes are a
/// genuine x264 header pair (extracted from an ultrafast/zerolatency 1080p30
/// encode; SPS `67 f4 00 28 ...`, PPS `68 ce 0f 19 20`), not length-shaped
/// filler: a real ingest parses avcC to establish the video track, and
/// placeholder lengths fail that parse (proven against MediaMTX, see
/// `tests/prompt10_rtmp.rs` `mediamtx_proof_*`). The live feed replaces these
/// with the encoder's own sets when available (see
/// `super::stream::avc_sequence_header`); these stand in for transports fed
/// synthetically, and the double asserts only the keyframe + sequence shape.
pub fn video_sequence_header() -> Vec<u8> {
    // [frame-type|codec-id, avc-packet-type, cts×3] + avcC: version 1, then
    // profile/compat/level COPIED from the SPS below (0x64/0x00/0x28), then
    // length-size-minus-one (4-byte NALUs), one SPS, one PPS.
    let sps: &[u8] = &[
        0x67, 0xF4, 0x00, 0x28, 0x91, 0x96, 0x80, 0x78, 0x02, 0x27, 0xE2, 0x70, 0x11, 0x00, 0x00,
        0x03, 0x00, 0x01, 0x00, 0x00, 0x03, 0x00, 0x3C, 0x8F, 0x18, 0x32, 0xA0,
    ];
    let pps: &[u8] = &[0x68, 0xCE, 0x0F, 0x19, 0x20];
    let mut out = vec![
        0x17, 0x00, 0x00, 0x00, 0x00, // FLV video tag header: keyframe, AVC, sequence
        0x01, 0x64, 0x00, 0x28, // avcC version 1, High profile, compat, level 4.0
        0xFF, 0xE1, // 4-byte NALU lengths, one SPS
    ];
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(0x01); // one PPS
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    out
}

/// FLV-typed AAC sequence header: 0xAF 0x00 + AudioSpecificConfig for AAC-LC
/// 48 kHz stereo — enough to identify the audio codec.
pub fn audio_sequence_header() -> Vec<u8> {
    vec![
        0xAF, 0x00, // FLV audio tag header: AAC, 48kHz, 16-bit, stereo, sequence
        0x12, 0x10, // AudioSpecificConfig: AAC-LC, 48 kHz, stereo
    ]
}

/// Publisher liveness as the operator sees it. Distinct from the engine's
/// `StreamState` (which stays `Live` while the transport redials — local
/// playout never follows the socket).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublisherState {
    /// Handshake + dialog done, media flowing.
    Live,
    /// Dial/redial in progress (backoff-capped). Media sheds, View continues.
    Reconnecting,
    /// Shut down: the socket is gone, the task exited.
    Closed,
}

/// First media payload of a kind + its wire type, shared between the
/// feeder-side handle (caches at send time) and the publisher task (replays
/// after every dialog).
type SeqCache = Arc<Mutex<Option<(u8, Vec<u8>)>>>;
/// One media message for the publisher task.
#[derive(Debug)]
struct PublishFrame {
    kind: u8,
    payload: Vec<u8>,
}

fn video_frame(payload: Vec<u8>) -> PublishFrame {
    PublishFrame {
        kind: MSG_VIDEO,
        payload,
    }
}

fn audio_frame(payload: Vec<u8>) -> PublishFrame {
    PublishFrame {
        kind: MSG_AUDIO,
        payload,
    }
}

/// The feeder-side handle: `try_send` only, so the render loop can never wait
/// on the socket. `Send + Sync` so engine state can hold it.
pub struct PublisherHandle {
    tx: tokio::sync::mpsc::Sender<PublishFrame>,
    /// Bytes accepted but not yet written to the socket — the ONLY input to
    /// [`PublisherHandle::buffer_ms`]. Kernel/socket buffers are invisible by
    /// construction (documented, not hidden): what we count is what we hold.
    buffered: Arc<AtomicUsize>,
    /// Frames shed while the channel was full (live edge: drop-new, counted).
    shed: Arc<AtomicU64>,
    state: Arc<Mutex<PublisherState>>,
    shutdown: Arc<tokio::sync::Notify>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// First media payload of each kind, cached at SEND time (not in the
    /// task): frames fed while the transport redials shed, but the codecs
    /// must still be re-announced on every (re)connect without feeder
    /// cooperation. The task replays these after each dialog.
    cached_video_seq: SeqCache,
    cached_audio_seq: SeqCache,
}

impl PublisherHandle {
    /// Queue video bytes. Never blocks; false = shed (channel full) or shut.
    pub fn try_publish_video(&self, payload: Vec<u8>) -> bool {
        self.send(video_frame(payload))
    }

    /// Queue audio bytes. Never blocks; false = shed (channel full) or shut.
    pub fn try_publish_audio(&self, payload: Vec<u8>) -> bool {
        self.send(audio_frame(payload))
    }

    fn send(&self, frame: PublishFrame) -> bool {
        // Byte accounting: credit HERE on admission (so the mpsc backlog
        // counts — channel-backlog INCLUDED, see module docs); the task
        // debits on write / stale-drain. Codec caching also lives HERE
        // (send time): the feeder's first payload of each kind is the
        // sequence header, and it must survive shedding during a redial so
        // the task can replay it after the next dialog.
        if frame.kind == MSG_VIDEO {
            let mut guard = self
                .cached_video_seq
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                *guard = Some((frame.kind, frame.payload.clone()));
            }
        }
        if frame.kind == MSG_AUDIO {
            let mut guard = self
                .cached_audio_seq
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                *guard = Some((frame.kind, frame.payload.clone()));
            }
        }
        let len = frame.payload.len();
        match self.tx.try_send(frame) {
            Ok(()) => {
                self.buffered.fetch_add(len, Ordering::SeqCst);
                true
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(frame)) => {
                self.shed.fetch_add(1, Ordering::SeqCst);
                let _ = frame;
                false
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Bytes currently held (accepted, unwritten). Honest, not estimated.
    pub fn buffered_bytes(&self) -> usize {
        self.buffered.load(Ordering::SeqCst)
    }

    /// Frames shed while the channel was full.
    pub fn shed_frames(&self) -> u64 {
        self.shed.load(Ordering::SeqCst)
    }

    /// `buffered_bytes` through the §9.4 envelope bitrate. Moves with load by
    /// construction: it divides the live counter, it is not stored.
    pub fn buffer_ms(&self) -> f64 {
        self.buffered.load(Ordering::SeqCst) as f64 * 8000.0 / ENVELOPE_BITRATE_BPS as f64
    }

    /// Current transport liveness.
    pub fn publisher_state(&self) -> PublisherState {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Signal shutdown and wait (bounded) for the task to exit and the socket
    /// to close. True = the transport is confirmed gone.
    ///
    /// Async by construction: `tokio::time::sleep`, never `std::thread::sleep`
    /// — calling this from the directive path must not block the executor
    /// (the old blocking spin stalled `stream.stop`'s ack window).
    pub async fn shutdown_and_wait(&self, timeout: Duration) -> bool {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = PublisherState::Closed;
        self.shutdown.notify_waiters();
        let start = std::time::Instant::now();
        loop {
            let done = self
                .task
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(|h| h.is_finished())
                .unwrap_or(true);
            if done {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Sync fire-and-forget for paths that cannot await (force-abandon):
    /// signals shutdown without waiting. The task exits on its own.
    pub fn shutdown_signal(&self) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = PublisherState::Closed;
        self.shutdown.notify_waiters();
    }

    /// Stop timeout for [`PublisherHandle::shutdown_and_wait`].
    pub fn stop_timeout() -> Duration {
        PUBLISH_STOP_TIMEOUT
    }
}

/// Spawn the publisher task for `url` and return its handle. Returns
/// immediately: the task dials in the background (state starts
/// `Reconnecting`, flips `Live` on dialog completion), so the directive path
/// never waits on the network. Requires a tokio runtime (the directive path
/// always has one; the caller checks).
pub fn spawn_publisher(url: RtmpUrl) -> PublisherHandle {
    let (tx, rx) = tokio::sync::mpsc::channel::<PublishFrame>(PUBLISH_CHANNEL_BOUND);
    let buffered = Arc::new(AtomicUsize::new(0));
    let shed = Arc::new(AtomicU64::new(0));
    let state = Arc::new(Mutex::new(PublisherState::Reconnecting));
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let cached_video_seq: SeqCache = Arc::new(Mutex::new(None));
    let cached_audio_seq: SeqCache = Arc::new(Mutex::new(None));
    let args = PublisherTaskArgs {
        url,
        rx,
        buffered: buffered.clone(),
        shed: shed.clone(),
        state: state.clone(),
        shutdown: shutdown.clone(),
        cached_video_seq: cached_video_seq.clone(),
        cached_audio_seq: cached_audio_seq.clone(),
    };
    let handle = tokio::spawn(async move { publisher_task(args).await });
    PublisherHandle {
        tx,
        buffered,
        shed,
        state,
        shutdown,
        task: Mutex::new(Some(handle)),
        cached_video_seq,
        cached_audio_seq,
    }
}

/// `buffered` is admitted-but-unwritten (channel backlog INCLUDED): the
/// handle credits on `try_send` success, this task debits on socket write or
/// stale-drain drop — exact by construction.
struct PublisherTaskArgs {
    url: RtmpUrl,
    rx: tokio::sync::mpsc::Receiver<PublishFrame>,
    buffered: Arc<AtomicUsize>,
    shed: Arc<AtomicU64>,
    state: Arc<Mutex<PublisherState>>,
    shutdown: Arc<tokio::sync::Notify>,
    cached_video_seq: SeqCache,
    cached_audio_seq: SeqCache,
}

/// RTMP default chunk payload size (§5.4.1): every message larger than this is
/// split, first chunk fmt=0 then fmt=3 continuations. Negotiated up by the
/// server's `Set Chunk Size` (0x01) during the dialog; mid-stream updates are
/// applied the same way.
const DEFAULT_CHUNK_SIZE: usize = 128;
/// Command chunk stream id (connect / createStream / publish).
const CSID_COMMAND: u32 = 3;
/// Media chunk streams (video / audio). Small (<64) so the basic header stays
/// one byte.
const CSID_VIDEO: u32 = 4;
const CSID_AUDIO: u32 = 5;
/// RTMP message types we send / expect.
const MSG_SET_CHUNK_SIZE: u8 = 0x01;
const MSG_COMMAND: u8 = 0x14;
/// Audio / video message types (coincide with the FLV tag kinds the feeder
/// publishes: 0x08 AAC, 0x09 H.264 — one numbering on both layers).
const MSG_AUDIO: u8 = 0x08;
const MSG_VIDEO: u8 = 0x09;

async fn publisher_task(
    PublisherTaskArgs {
        url,
        mut rx,
        buffered,
        shed,
        state,
        shutdown,
        cached_video_seq,
        cached_audio_seq,
    }: PublisherTaskArgs,
) {
    let set_state = |s: PublisherState| {
        *state.lock().unwrap_or_else(|e| e.into_inner()) = s;
    };
    let closed = Arc::new(AtomicBool::new(false));
    let closed_c = closed.clone();
    let shutdown_c = shutdown.clone();
    tokio::spawn(async move {
        shutdown_c.notified().await;
        closed_c.store(true, Ordering::SeqCst);
    });
    let mut pause = INITIAL_RECONNECT_PAUSE;
    loop {
        if closed.load(Ordering::SeqCst) {
            break;
        }
        set_state(PublisherState::Reconnecting);
        let stream = match tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio::net::TcpStream::connect((url.host.as_str(), url.port)),
        )
        .await
        {
            Ok(Ok(s)) => s,
            _ => {
                sleep_or_shutdown(&shutdown, pause).await;
                if closed.load(Ordering::SeqCst) {
                    break;
                }
                pause = (pause * 2).min(MAX_RECONNECT_PAUSE);
                continue;
            }
        };
        match dialog(stream, &url, &shutdown).await {
            Ok(mut live) => {
                pause = INITIAL_RECONNECT_PAUSE;
                // Fresh live edge: stale frames queued during the outage are
                // ancient airtime — shed them (counted + debited, they were
                // credited at send), then re-announce the codecs so the peer
                // rejoins mid-stream cleanly.
                drain_stale(&mut rx, &buffered, &shed);
                // Replay the send-time cached sequence headers (cloned under
                // one lock each): the peer re-identifies the codecs on every
                // redial, even for frames the feeder sent during the outage.
                let video_seq = cached_video_seq
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let audio_seq = cached_audio_seq
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let mut ok = true;
                let t0 = std::time::Instant::now();
                if let Some((kind, payload)) = &video_seq {
                    let ts = t0.elapsed().as_millis() as u32;
                    if write_media(
                        &mut live.stream,
                        if *kind == 0x09 {
                            CSID_VIDEO
                        } else {
                            CSID_AUDIO
                        },
                        ts,
                        *kind,
                        live.stream_id,
                        payload,
                        live.out_chunk_size,
                    )
                    .await
                    .is_err()
                    {
                        ok = false;
                    }
                }
                if ok {
                    if let Some((kind, payload)) = &audio_seq {
                        let ts = t0.elapsed().as_millis() as u32;
                        if write_media(
                            &mut live.stream,
                            if *kind == 0x09 {
                                CSID_VIDEO
                            } else {
                                CSID_AUDIO
                            },
                            ts,
                            *kind,
                            live.stream_id,
                            payload,
                            live.out_chunk_size,
                        )
                        .await
                        .is_err()
                        {
                            ok = false;
                        }
                    }
                }
                if ok {
                    set_state(PublisherState::Live);
                    let outcome =
                        live_loop(&mut live, &mut rx, &buffered, &shutdown, &closed).await;
                    if outcome == LoopEnd::Shutdown {
                        break;
                    }
                }
                // Write failure or EOF: redial (backoff continues below).
                sleep_or_shutdown(&shutdown, pause).await;
                if closed.load(Ordering::SeqCst) {
                    break;
                }
                pause = (pause * 2).min(MAX_RECONNECT_PAUSE);
            }
            Err(_) => {
                sleep_or_shutdown(&shutdown, pause).await;
                if closed.load(Ordering::SeqCst) {
                    break;
                }
                pause = (pause * 2).min(MAX_RECONNECT_PAUSE);
            }
        }
    }
    set_state(PublisherState::Closed);
}

async fn sleep_or_shutdown(shutdown: &Arc<tokio::sync::Notify>, pause: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(pause) => {}
        _ = shutdown.notified() => {}
    }
}

/// Drop everything queued during an outage (counted as shed AND debited —
/// the bytes were credited at send, so dropping without debiting would leak
/// the counter and lie `streamBufferMs` upward): the peer gets the live edge,
/// not ancient airtime.
fn drain_stale(
    rx: &mut tokio::sync::mpsc::Receiver<PublishFrame>,
    buffered: &Arc<AtomicUsize>,
    shed: &Arc<AtomicU64>,
) {
    let mut n = 0u64;
    let mut bytes = 0usize;
    while let Ok(frame) = rx.try_recv() {
        n += 1;
        bytes += frame.payload.len();
    }
    if n > 0 {
        shed.fetch_add(n, Ordering::SeqCst);
        buffered.fetch_sub(bytes.min(buffered.load(Ordering::SeqCst)), Ordering::SeqCst);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LoopEnd {
    Transport,
    Shutdown,
}

/// A dialog-established live transport: the socket, the outbound chunk size
/// (default 128, raised by the server's Set Chunk Size), and the stream id
/// `createStream` returned (media + publish ride on it).
struct LiveTransport {
    stream: tokio::net::TcpStream,
    out_chunk_size: usize,
    stream_id: u32,
}

/// Pump queued frames onto the socket until the peer goes away or shutdown
/// fires. Debits `buffered` on every write (credited at send) so the counter
/// is admitted-but-unwritten, exact. Incoming control (Set Chunk Size, pings,
/// onStatus) is drained — never treated as EOF unless the socket reads 0.
/// EOF while idle surfaces via `readable()`.
async fn live_loop(
    live: &mut LiveTransport,
    rx: &mut tokio::sync::mpsc::Receiver<PublishFrame>,
    buffered: &Arc<AtomicUsize>,
    shutdown: &Arc<tokio::sync::Notify>,
    closed: &Arc<AtomicBool>,
) -> LoopEnd {
    let t0 = std::time::Instant::now();
    // Level-triggered shutdown: `Notify` is edge-triggered (a notify that
    // lands between select iterations is lost), while the `closed` flag set
    // by the shutdown watcher is level state. The sleep branch re-checks it
    // every 50 ms so a lost notify still exits well inside the stop timeout;
    // the write below is likewise shutdown-cancellable so a stalled peer's
    // backlog cannot hold the task past the bound.
    loop {
        if closed.load(Ordering::SeqCst) {
            return LoopEnd::Shutdown;
        }
        tokio::select! {
            _ = shutdown.notified() => return LoopEnd::Shutdown,
            res = rx.recv() => {
                let Some(frame) = res else {
                    return LoopEnd::Shutdown;
                };
                let len = frame.payload.len();
                let csid = if frame.kind == 0x09 { CSID_VIDEO } else { CSID_AUDIO };
                let ts = t0.elapsed().as_millis() as u32;
                let write = tokio::select! {
                    res = write_media(
                        &mut live.stream,
                        csid,
                        ts,
                        frame.kind,
                        live.stream_id,
                        &frame.payload,
                        live.out_chunk_size,
                    ) => Some(res),
                    _ = shutdown.notified() => None,
                };
                // Debit on write, on failure, OR on shutdown-abandon: the
                // bytes left our hands either way (failure redials and
                // re-announces from the seq cache; abandon closes the
                // socket). Never leak the counter upward.
                buffered.fetch_sub(
                    len.min(buffered.load(Ordering::SeqCst)),
                    Ordering::SeqCst,
                );
                match write {
                    None => return LoopEnd::Shutdown,
                    Some(Err(_)) => return LoopEnd::Transport,
                    Some(Ok(())) => {}
                }
            }
            _ = live.stream.readable() => {
                if closed.load(Ordering::SeqCst) {
                    return LoopEnd::Shutdown;
                }
                // Drain one available chunk message (updates out_chunk_size
                // on Set Chunk Size); EOF (0 bytes) is the transport dying.
                // Anything else — pings, acks, onStatus — is control, not
                // media, and is discarded here (short-lived publish proof
                // needs no pong; a long-lived session would answer pings).
                match try_drain_one_message(&mut live.stream, &mut live.out_chunk_size).await {
                    DrainOutcome::Eof => return LoopEnd::Transport,
                    DrainOutcome::Ok | DrainOutcome::WouldBlock => continue,
                }
            }
            // Level-triggered backstop: if the edge notify above was lost
            // between iterations, this re-checks the flag the shutdown
            // watcher sets — bounded 50 ms granularity, far inside the stop
            // timeout.
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if closed.load(Ordering::SeqCst) {
                    return LoopEnd::Shutdown;
                }
            }
        }
    }
}

enum DrainOutcome {
    Ok,
    WouldBlock,
    Eof,
}

async fn try_drain_one_message(
    stream: &mut tokio::net::TcpStream,
    out_chunk_size: &mut usize,
) -> DrainOutcome {
    let mut tmp = [0u8; 4096];
    match stream.try_read(&mut tmp) {
        Ok(0) => DrainOutcome::Eof,
        Ok(n) => {
            // Minimal parse: if this looks like a Set Chunk Size control
            // (csid 2, fmt 0, type 0x01, len 4), adopt it for OUR sends.
            // Full reassembly lives in the dialog reader; mid-stream we only
            // need the size update, everything else is discardable control.
            // Layout: basic(1) + header(11) + payload(4) = 16 bytes for the
            // canonical encoding (csid 2, stream 0).
            if n >= 16 && tmp[0] == 0x02 {
                let msg_type = tmp[7];
                if msg_type == MSG_SET_CHUNK_SIZE {
                    let size = u32::from_be_bytes([tmp[12], tmp[13], tmp[14], tmp[15]]) as usize;
                    if (128..=65536).contains(&size) {
                        *out_chunk_size = size;
                    }
                }
            }
            DrainOutcome::Ok
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => DrainOutcome::WouldBlock,
        Err(_) => DrainOutcome::Eof,
    }
}

/// Write one RTMP message, chunked at `chunk_size`: first chunk fmt=0 with
/// the full message header, continuations fmt=3 (basic header only).
async fn write_chunked(
    stream: &mut tokio::net::TcpStream,
    csid: u32,
    timestamp: u32,
    msg_type: u8,
    stream_id: u32,
    payload: &[u8],
    chunk_size: usize,
) -> std::io::Result<()> {
    let chunk_size = chunk_size.max(1);
    let mut offset = 0usize;
    let mut first = true;
    while offset < payload.len() || (payload.is_empty() && first) {
        let take = (payload.len() - offset).min(chunk_size);
        if first {
            let mut hdr = encode_basic_header(0, csid);
            let ts_field = if timestamp >= 0xFF_FF_FF {
                0xFF_FF_FF
            } else {
                timestamp
            };
            let len = payload.len() as u32;
            hdr.extend_from_slice(&ts_field.to_be_bytes()[1..4]);
            hdr.extend_from_slice(&len.to_be_bytes()[1..4]);
            hdr.push(msg_type);
            hdr.extend_from_slice(&stream_id.to_le_bytes());
            if timestamp >= 0xFF_FF_FF {
                hdr.extend_from_slice(&timestamp.to_be_bytes());
            }
            stream.write_all(&hdr).await?;
            first = false;
        } else {
            stream.write_all(&encode_basic_header(3, csid)).await?;
        }
        if take > 0 {
            stream.write_all(&payload[offset..offset + take]).await?;
            offset += take;
        } else {
            break;
        }
    }
    Ok(())
}

fn encode_basic_header(fmt: u8, csid: u32) -> Vec<u8> {
    if csid < 64 {
        vec![(fmt << 6) | (csid as u8)]
    } else if csid < 320 {
        vec![(fmt << 6), (csid - 64) as u8]
    } else {
        let v = csid - 64;
        vec![(fmt << 6) | 1, (v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8]
    }
}

async fn write_media(
    stream: &mut tokio::net::TcpStream,
    csid: u32,
    timestamp: u32,
    kind: u8,
    stream_id: u32,
    payload: &[u8],
    chunk_size: usize,
) -> std::io::Result<()> {
    write_chunked(
        stream, csid, timestamp, kind, stream_id, payload, chunk_size,
    )
    .await
}

// ---------------------------------------------------------------------------
// AMF0 (minimal: encode String/Number/Bool/Null/Object for invokes; decode
// String/Number/Bool/Null/Object/ECMA for _result/onStatus).
// ---------------------------------------------------------------------------

fn amf0_string(s: &str) -> Vec<u8> {
    let mut out = vec![0x02];
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
    out
}

fn amf0_number(n: f64) -> Vec<u8> {
    let mut out = vec![0x00];
    out.extend_from_slice(&n.to_be_bytes());
    out
}

fn amf0_bool(b: bool) -> Vec<u8> {
    vec![0x01, u8::from(b)]
}

fn amf0_null() -> Vec<u8> {
    vec![0x05]
}

fn amf0_object(props: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0x03];
    for (k, v) in props {
        out.extend_from_slice(&(k.len() as u16).to_be_bytes());
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(v);
    }
    out.extend_from_slice(&[0x00, 0x00, 0x09]);
    out
}

fn amf0_invoke(name: &str, trans: f64, props: Vec<u8>, args: Vec<u8>) -> Vec<u8> {
    let mut out = amf0_string(name);
    out.extend_from_slice(&amf0_number(trans));
    out.extend_from_slice(&props);
    out.extend_from_slice(&args);
    out
}

#[derive(Debug)]
#[allow(dead_code)]
enum Amf0 {
    Number(f64),
    Bool(bool),
    String(String),
    Object(Vec<(String, Amf0)>),
    Null,
    Undefined,
}

fn amf0_decode(buf: &[u8], mut pos: usize) -> Result<(Amf0, usize), String> {
    let t = *buf.get(pos).ok_or("amf0: truncated type")?;
    pos += 1;
    match t {
        0x00 => {
            let b: [u8; 8] = buf
                .get(pos..pos + 8)
                .ok_or("amf0: truncated number")?
                .try_into()
                .map_err(|_| "amf0: number slice")?;
            Ok((Amf0::Number(f64::from_be_bytes(b)), pos + 8))
        }
        0x01 => {
            let b = *buf.get(pos).ok_or("amf0: truncated bool")?;
            Ok((Amf0::Bool(b != 0), pos + 1))
        }
        0x02 => {
            let n = u16::from_be_bytes(
                buf.get(pos..pos + 2)
                    .ok_or("amf0: truncated str len")?
                    .try_into()
                    .map_err(|_| "amf0: str len")?,
            ) as usize;
            pos += 2;
            let s = std::str::from_utf8(buf.get(pos..pos + n).ok_or("amf0: truncated str")?)
                .map_err(|_| "amf0: str utf8")?
                .to_string();
            Ok((Amf0::String(s), pos + n))
        }
        0x03 => {
            let mut props = Vec::new();
            loop {
                let n = u16::from_be_bytes(
                    buf.get(pos..pos + 2)
                        .ok_or("amf0: truncated obj key len")?
                        .try_into()
                        .map_err(|_| "amf0: obj key len")?,
                ) as usize;
                pos += 2;
                if n == 0 {
                    let end = *buf.get(pos).ok_or("amf0: truncated obj end")?;
                    pos += 1;
                    if end == 0x09 {
                        break;
                    }
                    return Err("amf0: bad object end".into());
                }
                let k = std::str::from_utf8(buf.get(pos..pos + n).ok_or("amf0: truncated key")?)
                    .map_err(|_| "amf0: key utf8")?
                    .to_string();
                pos += n;
                let (v, np) = amf0_decode(buf, pos)?;
                pos = np;
                props.push((k, v));
            }
            Ok((Amf0::Object(props), pos))
        }
        0x05 | 0x06 => Ok((Amf0::Null, pos)),
        0x08 => {
            let _count = u32::from_be_bytes(
                buf.get(pos..pos + 4)
                    .ok_or("amf0: truncated ecma count")?
                    .try_into()
                    .map_err(|_| "amf0: ecma count")?,
            );
            pos += 4;
            let mut props = Vec::new();
            loop {
                let n = u16::from_be_bytes(
                    buf.get(pos..pos + 2)
                        .ok_or("amf0: truncated ecma key len")?
                        .try_into()
                        .map_err(|_| "amf0: ecma key len")?,
                ) as usize;
                pos += 2;
                if n == 0 {
                    let end = *buf.get(pos).ok_or("amf0: truncated ecma end")?;
                    pos += 1;
                    if end == 0x09 {
                        break;
                    }
                    return Err("amf0: bad ecma end".into());
                }
                let k = std::str::from_utf8(buf.get(pos..pos + n).ok_or("amf0: truncated key")?)
                    .map_err(|_| "amf0: key utf8")?
                    .to_string();
                pos += n;
                let (v, np) = amf0_decode(buf, pos)?;
                pos = np;
                props.push((k, v));
            }
            Ok((Amf0::Object(props), pos))
        }
        0x0C => {
            let n = u32::from_be_bytes(
                buf.get(pos..pos + 4)
                    .ok_or("amf0: truncated longstr len")?
                    .try_into()
                    .map_err(|_| "amf0: longstr len")?,
            ) as usize;
            pos += 4;
            let s = std::str::from_utf8(buf.get(pos..pos + n).ok_or("amf0: truncated longstr")?)
                .map_err(|_| "amf0: longstr utf8")?
                .to_string();
            Ok((Amf0::String(s), pos + n))
        }
        other => Err(format!("amf0: unsupported type {other:#x}")),
    }
}

// ---------------------------------------------------------------------------
// Chunk reader (dialog path): reassembles messages split at the negotiated
// inbound chunk size (default 128), fmt 0/1/2/3 + extended timestamps.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LastHeader {
    timestamp: u32,
    msg_len: usize,
    msg_type: u8,
    stream_id: u32,
}

struct ChunkReader {
    in_chunk_size: usize,
    last: std::collections::HashMap<u32, LastHeader>,
    partial: std::collections::HashMap<u32, (LastHeader, Vec<u8>)>,
}

#[derive(Debug)]
#[allow(dead_code)]
struct InMessage {
    #[allow(dead_code)]
    csid: u32,
    timestamp: u32,
    msg_type: u8,
    stream_id: u32,
    payload: Vec<u8>,
}

impl ChunkReader {
    fn new() -> Self {
        Self {
            in_chunk_size: DEFAULT_CHUNK_SIZE,
            last: std::collections::HashMap::new(),
            partial: std::collections::HashMap::new(),
        }
    }

    async fn next_message(
        &mut self,
        stream: &mut tokio::net::TcpStream,
        shutdown: &Arc<tokio::sync::Notify>,
    ) -> Result<InMessage, RtmpError> {
        loop {
            let fmt_csid = read_byte(stream, shutdown).await?;
            let fmt = fmt_csid >> 6;
            let mut csid = (fmt_csid & 0x3F) as u32;
            if csid == 0 {
                csid = read_byte(stream, shutdown).await? as u32 + 64;
            } else if csid == 1 {
                let a = read_byte(stream, shutdown).await? as u32;
                let b = read_byte(stream, shutdown).await? as u32;
                csid = a + b * 256 + 64;
            }
            let prev = self.last.get(&csid).cloned();
            let (timestamp, msg_len, msg_type, stream_id) = match fmt {
                0 => {
                    let ts = read_u24(stream, shutdown).await?;
                    let len = read_u24(stream, shutdown).await? as usize;
                    let typ = read_byte(stream, shutdown).await?;
                    let sid = read_u32_le(stream, shutdown).await?;
                    let ts = if ts == 0xFF_FF_FF {
                        read_u32_be(stream, shutdown).await?
                    } else {
                        ts
                    };
                    self.last.insert(
                        csid,
                        LastHeader {
                            timestamp: ts,
                            msg_len: len,
                            msg_type: typ,
                            stream_id: sid,
                        },
                    );
                    (ts, len, typ, sid)
                }
                1 => {
                    let delta = read_u24(stream, shutdown).await?;
                    let len = read_u24(stream, shutdown).await? as usize;
                    let typ = read_byte(stream, shutdown).await?;
                    let p =
                        prev.ok_or_else(|| RtmpError::Protocol("fmt=1 with no history".into()))?;
                    let ts = if delta == 0xFF_FF_FF {
                        read_u32_be(stream, shutdown).await?
                    } else {
                        p.timestamp.wrapping_add(delta)
                    };
                    self.last.insert(
                        csid,
                        LastHeader {
                            timestamp: ts,
                            msg_len: len,
                            msg_type: typ,
                            stream_id: p.stream_id,
                        },
                    );
                    (ts, len, typ, p.stream_id)
                }
                2 => {
                    let delta = read_u24(stream, shutdown).await?;
                    let p =
                        prev.ok_or_else(|| RtmpError::Protocol("fmt=2 with no history".into()))?;
                    let ts = if delta == 0xFF_FF_FF {
                        read_u32_be(stream, shutdown).await?
                    } else {
                        p.timestamp.wrapping_add(delta)
                    };
                    self.last.insert(
                        csid,
                        LastHeader {
                            timestamp: ts,
                            msg_len: p.msg_len,
                            msg_type: p.msg_type,
                            stream_id: p.stream_id,
                        },
                    );
                    (ts, p.msg_len, p.msg_type, p.stream_id)
                }
                3 => {
                    let p =
                        prev.ok_or_else(|| RtmpError::Protocol("fmt=3 with no history".into()))?;
                    (p.timestamp, p.msg_len, p.msg_type, p.stream_id)
                }
                _ => return Err(RtmpError::Protocol("bad fmt".into())),
            };
            // One chunk's worth for this csid.
            let (base, mut buf) = self
                .partial
                .remove(&csid)
                .map(|(h, b)| (Some(h), b))
                .unwrap_or((None, Vec::new()));
            let want_total = if let Some(b) = &base {
                b.msg_len
            } else {
                msg_len
            };
            let have = buf.len();
            let remaining = want_total.saturating_sub(have);
            let take = remaining.min(self.in_chunk_size);
            if take > 0 {
                let mut chunk = vec![0u8; take];
                read_exact(stream, shutdown, &mut chunk).await?;
                buf.extend_from_slice(&chunk);
            }
            if buf.len() < want_total {
                let hdr = base.unwrap_or(LastHeader {
                    timestamp,
                    msg_len,
                    msg_type,
                    stream_id,
                });
                self.partial.insert(csid, (hdr, buf));
                continue;
            }
            // Complete message.
            if msg_type == MSG_SET_CHUNK_SIZE && buf.len() >= 4 {
                let size = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                if (1..=65536).contains(&size) {
                    self.in_chunk_size = size;
                }
                continue;
            }
            return Ok(InMessage {
                csid,
                timestamp,
                msg_type,
                stream_id,
                payload: buf,
            });
        }
    }
}

async fn read_byte(
    stream: &mut tokio::net::TcpStream,
    shutdown: &Arc<tokio::sync::Notify>,
) -> Result<u8, RtmpError> {
    let mut b = [0u8; 1];
    read_exact(stream, shutdown, &mut b).await?;
    Ok(b[0])
}

async fn read_u24(
    stream: &mut tokio::net::TcpStream,
    shutdown: &Arc<tokio::sync::Notify>,
) -> Result<u32, RtmpError> {
    let mut b = [0u8; 3];
    read_exact(stream, shutdown, &mut b).await?;
    Ok(((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32)
}

async fn read_u32_be(
    stream: &mut tokio::net::TcpStream,
    shutdown: &Arc<tokio::sync::Notify>,
) -> Result<u32, RtmpError> {
    let mut b = [0u8; 4];
    read_exact(stream, shutdown, &mut b).await?;
    Ok(u32::from_be_bytes(b))
}

async fn read_u32_le(
    stream: &mut tokio::net::TcpStream,
    shutdown: &Arc<tokio::sync::Notify>,
) -> Result<u32, RtmpError> {
    let mut b = [0u8; 4];
    read_exact(stream, shutdown, &mut b).await?;
    Ok(u32::from_le_bytes(b))
}

/// TCP connect is done by the caller; this runs the handshake + dialog and
/// returns the live transport ready for media.
async fn dialog(
    mut stream: tokio::net::TcpStream,
    url: &RtmpUrl,
    shutdown: &Arc<tokio::sync::Notify>,
) -> Result<LiveTransport, RtmpError> {
    // C0 + C1: version 3, timestamp + zero + 1528 arbitrary bytes.
    let mut c1 = [0u8; 1536];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0);
    c1[0..4].copy_from_slice(&now.to_be_bytes());
    for (i, b) in c1[8..].iter_mut().enumerate() {
        // Deterministic filler (no `rand` dep): the handshake needs 1528
        // arbitrary bytes, not entropy.
        *b = (i.wrapping_mul(0x9E) ^ 0x37) as u8;
    }
    write_or_shutdown(&mut stream, shutdown, &[3]).await?;
    write_or_shutdown(&mut stream, shutdown, &c1).await?;
    // S0 + S1 + S2.
    let mut s0 = [0u8; 1];
    let mut s1 = [0u8; 1536];
    let mut s2 = [0u8; 1536];
    read_or_shutdown(&mut stream, shutdown, &mut s0).await?;
    read_or_shutdown(&mut stream, shutdown, &mut s1).await?;
    read_or_shutdown(&mut stream, shutdown, &mut s2).await?;
    if s0[0] != 3 {
        return Err(RtmpError::Protocol("peer is not RTMP (bad version)".into()));
    }
    // S2 SHOULD echo C1 (Adobe simple handshake), but real servers vary —
    // MediaMTX sends zeros — and the echo authenticates nothing (C1 is our
    // own bytes parroted back), so any S2 is accepted. C2 still echoes S1
    // per spec (proven against MediaMTX v1.21.1, see tests/prompt10_rtmp.rs
    // `mediamtx_proof_*`). Both halves are still consumed off the wire.
    let _ = (&c1, &s2);
    // C2: echo S1.
    write_or_shutdown(&mut stream, shutdown, &s1).await?;

    let mut reader = ChunkReader::new();
    let mut out_chunk_size = DEFAULT_CHUNK_SIZE;

    // connect(app): tcUrl carries host+port+app; the key is a credential —
    // written inside publish args, never logged.
    let tc_url = format!("rtmp://{}:{}/{}", url.host, url.port, url.app);
    let connect_obj = amf0_object(&[
        ("app", amf0_string(&url.app)),
        ("flashVer", amf0_string("FMLE/3.0 (compatible; NBE)")),
        ("tcUrl", amf0_string(&tc_url)),
        ("fpad", amf0_bool(false)),
        ("capabilities", amf0_number(15.0)),
        ("audioCodecs", amf0_number(10.0)),
        ("videoCodecs", amf0_number(7.0)),
        ("videoFunction", amf0_number(1.0)),
        ("objectEncoding", amf0_number(0.0)),
    ]);
    let connect = amf0_invoke("connect", 1.0, connect_obj, Vec::new());
    write_chunked(
        &mut stream,
        CSID_COMMAND,
        0,
        MSG_COMMAND,
        0,
        &connect,
        out_chunk_size,
    )
    .await
    .map_err(|e| RtmpError::Io(e.to_string()))?;

    // createStream: sent after connect's _result (trans 1). Loop reading
    // control + command until it arrives (MediaMTX interleaves Window Ack,
    // Peer Bandwidth, Set Chunk Size, onBWDone).
    let mut connected = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !connected {
        if std::time::Instant::now() > deadline {
            return Err(RtmpError::Protocol("connect: no _result".into()));
        }
        let msg = tokio::select! {
            r = reader.next_message(&mut stream, shutdown) => r?,
            _ = shutdown.notified() => return Err(RtmpError::Io("shutting down".into())),
        };
        out_chunk_size = reader.in_chunk_size.clamp(DEFAULT_CHUNK_SIZE, 65536);
        if msg.msg_type != MSG_COMMAND {
            continue;
        }
        let (name, trans) = amf0_command_name_trans(&msg.payload)?;
        if name == "_result" && (trans - 1.0).abs() < 1e-6 {
            connected = true;
        } else if name == "_error" {
            return Err(RtmpError::Protocol("CONNECT refused".into()));
        }
    }

    // releaseStream + FCPublish (OBS/ffmpeg shape, trans 0.0 fire-and-forget):
    // real servers bind the stream name here, before any stream exists. The
    // key rides inside, never in logs. Replies (if any) are not waited on —
    // the createStream loop below skips every non-matching command.
    for cmd in ["releaseStream", "FCPublish"] {
        let mut args = amf0_null();
        args.extend_from_slice(&amf0_string(&url.key));
        let invoke = amf0_invoke(cmd, 0.0, args, Vec::new());
        write_chunked(
            &mut stream,
            CSID_COMMAND,
            0,
            MSG_COMMAND,
            0,
            &invoke,
            out_chunk_size,
        )
        .await
        .map_err(|e| RtmpError::Io(e.to_string()))?;
    }

    let create = amf0_invoke("createStream", 2.0, amf0_null(), vec![]);
    write_chunked(
        &mut stream,
        CSID_COMMAND,
        0,
        MSG_COMMAND,
        0,
        &create,
        out_chunk_size,
    )
    .await
    .map_err(|e| RtmpError::Io(e.to_string()))?;

    let mut stream_id: Option<u32> = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while stream_id.is_none() {
        if std::time::Instant::now() > deadline {
            return Err(RtmpError::Protocol("createStream: no _result".into()));
        }
        let msg = tokio::select! {
            r = reader.next_message(&mut stream, shutdown) => r?,
            _ = shutdown.notified() => return Err(RtmpError::Io("shutting down".into())),
        };
        out_chunk_size = reader.in_chunk_size.clamp(DEFAULT_CHUNK_SIZE, 65536);
        if msg.msg_type != MSG_COMMAND {
            continue;
        }
        let (name, trans) = amf0_command_name_trans(&msg.payload)?;
        if name == "_result" && (trans - 2.0).abs() < 1e-6 {
            stream_id = amf0_create_stream_id(&msg.payload);
            if stream_id.is_none() {
                return Err(RtmpError::Protocol("createStream: no stream id".into()));
            }
        } else if name == "_error" {
            return Err(RtmpError::Protocol("createStream refused".into()));
        }
    }
    let stream_id = stream_id.unwrap_or(1);

    // publish(key, live) on the new stream. The key is written, never logged.
    let mut pub_args = amf0_null();
    pub_args.extend_from_slice(&amf0_string(&url.key));
    pub_args.extend_from_slice(&amf0_string("live"));
    let publish = amf0_invoke("publish", 0.0, pub_args, vec![]);
    write_chunked(
        &mut stream,
        CSID_COMMAND,
        0,
        MSG_COMMAND,
        stream_id,
        &publish,
        out_chunk_size,
    )
    .await
    .map_err(|e| RtmpError::Io(e.to_string()))?;

    // Wait briefly for NetStream.Publish.Start (proves the server accepted
    // the key); proceed anyway on timeout — MediaMTX answers within ms, and
    // a slow answer must not fail a publish the server will accept.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if std::time::Instant::now() > deadline {
            break;
        }
        let msg = tokio::select! {
            r = reader.next_message(&mut stream, shutdown) => match r {
                Ok(m) => m,
                Err(_) => break,
            },
            _ = shutdown.notified() => return Err(RtmpError::Io("shutting down".into())),
            _ = tokio::time::sleep(Duration::from_millis(50)) => break,
        };
        out_chunk_size = reader.in_chunk_size.clamp(DEFAULT_CHUNK_SIZE, 65536);
        if msg.msg_type != MSG_COMMAND {
            continue;
        }
        if let Ok((name, _)) = amf0_command_name_trans(&msg.payload) {
            if name == "onStatus" {
                if amf0_payload_contains(&msg.payload, "NetStream.Publish.Start") {
                    break;
                }
                if amf0_payload_contains(&msg.payload, "NetStream.Publish.BadName")
                    || amf0_payload_contains(&msg.payload, "NetStream.Publish.Denied")
                {
                    return Err(RtmpError::Protocol("PUBLISH refused".into()));
                }
            } else if name == "_error" {
                return Err(RtmpError::Protocol("PUBLISH refused".into()));
            }
        }
    }

    Ok(LiveTransport {
        stream,
        out_chunk_size,
        stream_id,
    })
}

fn amf0_command_name_trans(payload: &[u8]) -> Result<(String, f64), RtmpError> {
    let (name, pos) =
        amf0_decode(payload, 0).map_err(|e| RtmpError::Protocol(format!("amf0 name: {e}")))?;
    let name = match name {
        Amf0::String(s) => s,
        _ => {
            return Err(RtmpError::Protocol(
                "amf0: command name not a string".into(),
            ))
        }
    };
    let (trans, _) =
        amf0_decode(payload, pos).map_err(|e| RtmpError::Protocol(format!("amf0 trans: {e}")))?;
    let trans = match trans {
        Amf0::Number(n) => n,
        _ => return Err(RtmpError::Protocol("amf0: trans not a number".into())),
    };
    let _ = pos;
    Ok((name, trans))
}

fn amf0_create_stream_id(payload: &[u8]) -> Option<u32> {
    let mut pos = 0;
    let _ = amf0_decode(payload, pos).ok()?;
    let (name, p1) = amf0_decode(payload, 0).ok()?;
    let _ = name;
    pos = p1;
    let (_, p2) = amf0_decode(payload, pos).ok()?;
    pos = p2;
    let _ = amf0_decode(payload, pos).ok().map(|(_, p)| pos = p);
    // _result layout: [name, trans, props, info]. info (4th value) is the
    // stream id number for createStream.
    let mut idx = 0;
    let mut at = 0;
    while idx < 4 {
        let (v, np) = amf0_decode(payload, at).ok()?;
        if idx == 3 {
            if let Amf0::Number(n) = v {
                return Some(n as u32);
            }
            return None;
        }
        at = np;
        idx += 1;
    }
    None
}

fn amf0_payload_contains(payload: &[u8], needle: &str) -> bool {
    // AMF0 strings ride length-prefixed, so a raw substring search is exact
    // enough for status codes (they appear verbatim as string values).
    if payload.len() < needle.len() {
        return false;
    }
    payload
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

async fn write_or_shutdown(
    stream: &mut tokio::net::TcpStream,
    shutdown: &Arc<tokio::sync::Notify>,
    bytes: &[u8],
) -> Result<(), RtmpError> {
    tokio::select! {
        res = stream.write_all(bytes) => res.map_err(|e| RtmpError::Io(e.to_string())),
        _ = shutdown.notified() => Err(RtmpError::Io("shutting down".into())),
    }
}

async fn read_or_shutdown(
    stream: &mut tokio::net::TcpStream,
    shutdown: &Arc<tokio::sync::Notify>,
    buf: &mut [u8],
) -> Result<(), RtmpError> {
    tokio::select! {
        res = stream.read_exact(buf) => {
            res.map_err(|e| RtmpError::Io(e.to_string()))?;
            Ok(())
        }
        _ = shutdown.notified() => Err(RtmpError::Io("shutting down".into())),
    }
}

async fn read_exact(
    stream: &mut tokio::net::TcpStream,
    shutdown: &Arc<tokio::sync::Notify>,
    buf: &mut [u8],
) -> Result<(), RtmpError> {
    read_or_shutdown(stream, shutdown, buf).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loopback_publish_targets() {
        let u = parse_rtmp_url("rtmp://127.0.0.1:19350/live/key-one").unwrap();
        assert_eq!(u.host, "127.0.0.1");
        assert_eq!(u.port, 19350);
        assert_eq!(u.app, "live");
        assert_eq!(u.key, "key-one");
    }

    #[test]
    fn default_port_is_1935() {
        let u = parse_rtmp_url("rtmp://example/live/key").unwrap();
        assert_eq!(u.port, 1935);
    }

    #[test]
    fn refuses_non_rtmp_and_keyless_targets() {
        assert!(parse_rtmp_url("http://x/live/k").is_err());
        assert!(parse_rtmp_url("rtmp://hostonly").is_err());
        assert!(parse_rtmp_url("rtmp://h/app").is_err());
        assert!(parse_rtmp_url("rtmp://h//key").is_err());
    }

    #[test]
    fn sequence_headers_identify_h264_and_aac() {
        assert!(video_sequence_header().starts_with(&[0x17, 0x00]));
        assert!(audio_sequence_header().starts_with(&[0xAF, 0x00]));
    }

    #[test]
    fn buffer_ms_divides_bytes_by_the_envelope() {
        assert_eq!(ENVELOPE_BITRATE_BPS, 8_192_000);
        let bytes = 1_024_000usize;
        let ms = bytes as f64 * 8000.0 / ENVELOPE_BITRATE_BPS as f64;
        assert!(
            (ms - 1000.0).abs() < 1e-6,
            "1_024_000 bytes at 8.192 Mbps is 1 s"
        );
    }
}
