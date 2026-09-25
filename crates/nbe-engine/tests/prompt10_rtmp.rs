//! Prompt 10 WU5 (SPEC §9.4, §9.5): RTMP transport, stream thread, survival.
//!
//! Two layers, split by what they need (PR #30 repair round):
//!
//! **Transport + stream thread — runner-independent, exercised on CI.** The
//! handshake, the AMF0 dialog, chunking, the extended timestamp, pings,
//! reconnect, buffer accounting, non-blocking publishes and the bounded stop
//! are properties of the transport, not of an H.264 encoder. PR #30 first
//! gated all thirteen of this suite's tests on the hardware encoder by
//! entering through `stream.start`, so CI (no encoder) exercised one of
//! them. Here they drive the real `PublisherHandle` (and, for audio and the
//! stop path, the real `StreamSession` and its thread) with synthetic
//! payloads, and run everywhere.
//!
//! **Live feed — hardware-gated, skips loudly.** Encoded video needs the
//! encoder and a zero-copy chain. These enter through the REAL directive path
//! (`show.load` → `show.start` → `stream.start`) and drive the REAL render
//! loop (`nbe_engine::tick::run_loop` — the function `main` calls), never a
//! harness imitating it (rule 7). PR #30's first live-feed tests called a
//! helper beside the loop; the loop's own stream leg was never entered.
//!
//! The double speaks REAL RTMP and behaves like a conforming server: it
//! reads the client's chunks at the size the CLIENT announced (never its
//! own), honours the extended timestamp on Type 3 continuations (RTMP
//! §5.3.1.3), announces its own outbound chunk size the way nginx-rtmp
//! (4096) and MediaMTX (65536) do, and can send a PingRequest. PR #30's first
//! double did none of these, which is how three transport bugs passed it.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::record::rtmp::{
    audio_sequence_header, parse_rtmp_url, spawn_publisher, spawn_publisher_with_envelope,
    video_sequence_header, PublisherHandle, PublisherState, ENVELOPE_BITRATE_BPS,
};
use nbe_engine::record::stream::{audio_ts_ms, StreamParams, StreamSession};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, OutgoingQueue, RecordState, StreamState};
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// Harness.
// ---------------------------------------------------------------------------

fn directive(command: &str, sv: u64, payload: serde_json::Value) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target: serde_json::json!({}),
        payload,
    }
}

fn harness_at(rate: u32) -> (Arc<EngineState>, DirectiveHandler, Arc<OutgoingQueue>) {
    let state = Arc::new(EngineState::new(rate));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    (state, handler, outgoing)
}

fn harness() -> (Arc<EngineState>, DirectiveHandler, Arc<OutgoingQueue>) {
    harness_at(30)
}

fn acked(outgoing: &OutgoingQueue, sv: u64) -> bool {
    outgoing.drain().into_iter().any(|f| match f {
        EngineFrame::AppliedStateVersion { state_version, .. } => state_version == sv,
        _ => false,
    })
}

/// The render loop for a hardware test, or a loud skip: the live feed needs
/// the encoder AND a zero-copy chain. The returned loop is the one the test
/// ticks — `RenderLoop::new` also publishes the device the chain probe uses.
async fn live_rig_or_skip(state: &Arc<EngineState>) -> Option<RenderLoop> {
    if !nbe_engine::record::encoder_available() {
        eprintln!("SKIP: no hardware H.264 encoder on this machine (SPEC §9.2)");
        return None;
    }
    let render = RenderLoop::new(state.clone()).await.ok()?;
    if !nbe_engine::record::stream::chain_available(&state.render_device()) {
        eprintln!("SKIP: no zero-copy chain on this machine (§0.1 assumption 24)");
        return None;
    }
    Some(render)
}

/// A minimal package whose `outputs.stream` is `stream` (merged over
/// `{ "url": url }`) at `rate` fps.
fn write_package_with(
    url: &str,
    rate: u32,
    stream: serde_json::Value,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let pkg = tempfile::tempdir().expect("package tempdir must succeed");
    std::fs::create_dir_all(pkg.path().join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(pkg.path().join("media/slate.png"), &png).unwrap();
    let mut output = serde_json::json!({ "url": url });
    if let serde_json::Value::Object(extra) = stream {
        for (k, v) in extra {
            output[k] = v;
        }
    }
    std::fs::write(
        pkg.path().join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.4",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 1920, "height": 1080, "frameRate": rate, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate",
                "outputs": { "stream": output }
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" }
            ],
            "scenes": [],
            "rundown": { "id": "R", "items": [] },
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
    let pkg_path = pkg.path().to_path_buf();
    (pkg, pkg_path)
}

fn write_package(url: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    write_package_with(url, 30, serde_json::json!({}))
}

/// Package variant with a record target: `record` is merged into
/// `outputs.record` beside the tempdir `directory`.
fn write_record_package(
    url: &str,
    record: serde_json::Value,
) -> (tempfile::TempDir, std::path::PathBuf, tempfile::TempDir) {
    let rec = tempfile::tempdir().expect("record tempdir must succeed");
    let (pkg, pkg_path) = write_package(url);
    let manifest_path = pkg_path.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    let mut out = serde_json::json!({ "directory": rec.path().to_string_lossy() });
    if let serde_json::Value::Object(extra) = record {
        for (k, v) in extra {
            out[k] = v;
        }
    }
    manifest["show"]["outputs"]["record"] = out;
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    (pkg, pkg_path, rec)
}

async fn load_and_start(handler: &DirectiveHandler, pkg_path: &std::path::Path) {
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": pkg_path.to_string_lossy() }),
        ))
        .await
        .expect("show.load of the test package must succeed");
    handler
        .apply(&directive("show.start", 2, serde_json::json!({})))
        .await
        .unwrap();
}

fn dropped(state: &Arc<EngineState>) -> u64 {
    state.dropped_frames_total.load(Ordering::SeqCst)
}

fn tick_stream_buffer_ms(state: &Arc<EngineState>) -> f64 {
    match nbe_engine::telemetry::build_tick_for_dir(state, None) {
        EngineFrame::EngineTelemetry { fields, .. } => fields.stream_buffer_ms,
        _ => panic!("build_tick_for_dir must emit engineTelemetry"),
    }
}

/// `streamTransportState` as the §10.1 tick carries it (SPEC v0.4.6).
fn tick_transport_state(state: &Arc<EngineState>) -> String {
    match nbe_engine::telemetry::build_tick_for_dir(state, None) {
        EngineFrame::EngineTelemetry { fields, .. } => fields.stream_transport_state,
        _ => panic!("build_tick_for_dir must emit engineTelemetry"),
    }
}

fn publisher_state_of(state: &Arc<EngineState>) -> PublisherState {
    state
        .stream_session
        .lock()
        .unwrap()
        .as_ref()
        .expect("stream session must be present")
        .publisher_state()
}

async fn poll_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cond()
}

/// Run the production render loop for `frames` ticks, returning the ticks'
/// reports (the loop's timed region per frame).
async fn run_frames(
    render: &mut RenderLoop,
    state: &Arc<EngineState>,
    rate: u32,
    frames: usize,
) -> Vec<nbe_engine::tick::TickReport> {
    let mut reports = Vec::with_capacity(frames);
    nbe_engine::tick::run_loop(render, state, rate, |r| {
        reports.push(*r);
        reports.len() < frames
    })
    .await;
    reports
}

fn zero_copy_selection() -> nbe_engine::record::tap_path::Selection {
    nbe_engine::record::tap_path::select_stream(true).expect("a chain selects zero-copy")
}

/// A synthetic publisher at `url` (no encoder, no engine): the transport
/// under test on its own.
fn publisher(url: &str) -> PublisherHandle {
    spawn_publisher(parse_rtmp_url(url).expect("test url must parse"))
}

async fn wait_publisher_live(p: &PublisherHandle) -> bool {
    poll_until(Duration::from_secs(5), || {
        p.publisher_state() == PublisherState::Live
    })
    .await
}

/// Admit one payload, retrying briefly: `try_send` is instant by design and
/// a synchronous test burst can outrun the drain. The NON-blocking property
/// itself is pinned by `publishes_never_block_under_backpressure`.
async fn admit(p: &PublisherHandle, video: bool, payload: Vec<u8>, ts: u32) {
    let start = Instant::now();
    loop {
        let ok = if video {
            p.try_publish_video(payload.clone(), ts)
        } else {
            p.try_publish_audio(payload.clone(), ts)
        };
        if ok {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "payload at ts {ts} was never admitted while Live"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// A recognisable payload: the FLV tag header then `len` bytes of a pattern
/// seeded by `seed`, so any misalignment in reassembly shows as a mismatch.
fn patterned(header: &[u8], len: usize, seed: u32) -> Vec<u8> {
    let mut out = header.to_vec();
    out.extend((0..len).map(|i| ((i as u32).wrapping_mul(31).wrapping_add(seed) % 251) as u8));
    out
}

// ---------------------------------------------------------------------------
// Minimal AMF0 encode/decode for the double (mirrors the client wire).
// ---------------------------------------------------------------------------

fn dbl_string(s: &str) -> Vec<u8> {
    let mut out = vec![0x02];
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
    out
}

fn dbl_number(n: f64) -> Vec<u8> {
    let mut out = vec![0x00];
    out.extend_from_slice(&n.to_be_bytes());
    out
}

fn dbl_null() -> Vec<u8> {
    vec![0x05]
}

fn dbl_object(props: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0x03];
    for (k, v) in props {
        out.extend_from_slice(&(k.len() as u16).to_be_bytes());
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(v);
    }
    out.extend_from_slice(&[0x00, 0x00, 0x09]);
    out
}

#[derive(Debug)]
enum DAmf {
    Number(f64),
    String(String),
    Object(Vec<(String, DAmf)>),
    Null,
}

fn damf_decode(buf: &[u8], mut pos: usize) -> Result<(DAmf, usize), String> {
    let t = *buf.get(pos).ok_or("truncated type")?;
    pos += 1;
    match t {
        0x00 => {
            let b: [u8; 8] = buf
                .get(pos..pos + 8)
                .ok_or("truncated number")?
                .try_into()
                .map_err(|_| "number slice")?;
            Ok((DAmf::Number(f64::from_be_bytes(b)), pos + 8))
        }
        0x01 => {
            let b = *buf.get(pos).ok_or("truncated bool")?;
            Ok((DAmf::Number(if b != 0 { 1.0 } else { 0.0 }), pos + 1))
        }
        0x02 => {
            let n = u16::from_be_bytes(
                buf.get(pos..pos + 2)
                    .ok_or("truncated str len")?
                    .try_into()
                    .map_err(|_| "str len")?,
            ) as usize;
            pos += 2;
            let s = std::str::from_utf8(buf.get(pos..pos + n).ok_or("truncated str")?)
                .map_err(|_| "str utf8")?
                .to_string();
            Ok((DAmf::String(s), pos + n))
        }
        0x03 => {
            let mut props = Vec::new();
            loop {
                let n = u16::from_be_bytes(
                    buf.get(pos..pos + 2)
                        .ok_or("truncated obj key len")?
                        .try_into()
                        .map_err(|_| "obj key len")?,
                ) as usize;
                pos += 2;
                if n == 0 {
                    let end = *buf.get(pos).ok_or("truncated obj end")?;
                    pos += 1;
                    if end == 0x09 {
                        break;
                    }
                    return Err("bad object end".into());
                }
                let k = std::str::from_utf8(buf.get(pos..pos + n).ok_or("truncated key")?)
                    .map_err(|_| "key utf8")?
                    .to_string();
                pos += n;
                let (v, np) = damf_decode(buf, pos)?;
                pos = np;
                props.push((k, v));
            }
            Ok((DAmf::Object(props), pos))
        }
        0x05 | 0x06 => Ok((DAmf::Null, pos)),
        0x08 => {
            pos += 4;
            let mut props = Vec::new();
            loop {
                let n = u16::from_be_bytes(
                    buf.get(pos..pos + 2)
                        .ok_or("truncated ecma key len")?
                        .try_into()
                        .map_err(|_| "ecma key len")?,
                ) as usize;
                pos += 2;
                if n == 0 {
                    let end = *buf.get(pos).ok_or("truncated ecma end")?;
                    pos += 1;
                    if end == 0x09 {
                        break;
                    }
                    return Err("bad ecma end".into());
                }
                let k = std::str::from_utf8(buf.get(pos..pos + n).ok_or("truncated key")?)
                    .map_err(|_| "key utf8")?
                    .to_string();
                pos += n;
                let (v, np) = damf_decode(buf, pos)?;
                pos = np;
                props.push((k, v));
            }
            Ok((DAmf::Object(props), pos))
        }
        0x0C => {
            let n = u32::from_be_bytes(
                buf.get(pos..pos + 4)
                    .ok_or("truncated longstr len")?
                    .try_into()
                    .map_err(|_| "longstr len")?,
            ) as usize;
            pos += 4;
            let s = std::str::from_utf8(buf.get(pos..pos + n).ok_or("truncated longstr")?)
                .map_err(|_| "longstr utf8")?
                .to_string();
            Ok((DAmf::String(s), pos + n))
        }
        other => Err(format!("unsupported amf0 type {other:#x}")),
    }
}

fn damf_command(payload: &[u8]) -> Option<(String, f64, Vec<DAmf>)> {
    let (name, mut pos) = damf_decode(payload, 0).ok()?;
    let name = match name {
        DAmf::String(s) => s,
        _ => return None,
    };
    let (trans, p2) = damf_decode(payload, pos).ok()?;
    let trans = match trans {
        DAmf::Number(n) => n,
        _ => return None,
    };
    pos = p2;
    let mut rest = Vec::new();
    while pos < payload.len() {
        let (v, np) = damf_decode(payload, pos).ok()?;
        pos = np;
        rest.push(v);
    }
    Some((name, trans, rest))
}

// ---------------------------------------------------------------------------
// In-process RTMP test double (pure std TCP, loopback only), conforming.
// ---------------------------------------------------------------------------

/// How the double behaves as a server.
#[derive(Debug, Clone, Copy)]
struct DoubleOpts {
    /// The double's own outbound chunk size, announced with Set Chunk Size
    /// when `connect` arrives (nginx-rtmp announces 4096, MediaMTX 65536).
    /// A client that adopts this for ITS sends breaks a conforming reader.
    announce_chunk: Option<u32>,
    /// Send a User Control PingRequest carrying this timestamp once publish
    /// starts (nginx-rtmp pings every 3 min and drops a silent publisher).
    ping_after_publish: Option<u32>,
}

impl Default for DoubleOpts {
    fn default() -> Self {
        Self {
            announce_chunk: Some(4096),
            ping_after_publish: None,
        }
    }
}

#[derive(Debug, Default)]
struct Received {
    app: String,
    key: String,
    handshake_ok: bool,
    connect_ok: bool,
    publish_ok: bool,
    /// Accepted connections (a redial is a second one).
    connections: u32,
    /// The chunk size the CLIENT announced for its sends, if any.
    client_chunk_size: Option<usize>,
    /// Every media message: (connection #, RTMP timestamp, payload).
    video: Vec<(u32, u32, Vec<u8>)>,
    audio: Vec<(u32, u32, Vec<u8>)>,
    /// The timestamp echoed in the client's PingResponse.
    pong: Option<u32>,
    /// The first reassembly failure: a conforming reader could not parse
    /// what the client sent.
    parse_error: Option<String>,
}

impl Received {
    fn is_video_seq(p: &[u8]) -> bool {
        p.starts_with(&[0x17, 0x00])
    }
    fn is_audio_seq(p: &[u8]) -> bool {
        p.starts_with(&[0xAF, 0x00])
    }
    fn audio_seq_payload(&self) -> Option<&[u8]> {
        self.audio
            .iter()
            .find(|(_, _, p)| Self::is_audio_seq(p))
            .map(|(_, _, p)| p.as_slice())
    }
    /// Media (non-sequence) video messages.
    fn video_media(&self) -> Vec<&(u32, u32, Vec<u8>)> {
        self.video
            .iter()
            .filter(|(_, _, p)| !Self::is_video_seq(p))
            .collect()
    }
    fn audio_media(&self) -> Vec<&(u32, u32, Vec<u8>)> {
        self.audio
            .iter()
            .filter(|(_, _, p)| !Self::is_audio_seq(p))
            .collect()
    }
    /// The first video payload a connection carried.
    fn first_video_on(&self, conn: u32) -> Option<&[u8]> {
        self.video
            .iter()
            .find(|(c, _, _)| *c == conn)
            .map(|(_, _, p)| p.as_slice())
    }
}

fn dbl_basic_header(fmt: u8, csid: u32) -> Vec<u8> {
    if csid < 64 {
        vec![(fmt << 6) | (csid as u8)]
    } else if csid < 320 {
        vec![(fmt << 6), (csid - 64) as u8]
    } else {
        let v = csid - 64;
        vec![(fmt << 6) | 1, (v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8]
    }
}

/// Write one message chunked at `chunk` (the double's announced size).
fn dbl_write_msg(
    s: &mut TcpStream,
    chunk: usize,
    csid: u32,
    msg_type: u8,
    stream_id: u32,
    payload: &[u8],
) -> std::io::Result<()> {
    let mut offset = 0usize;
    let mut first = true;
    while offset < payload.len() || (payload.is_empty() && first) {
        let take = (payload.len() - offset).min(chunk);
        if first {
            let mut hdr = dbl_basic_header(0, csid);
            hdr.extend_from_slice(&[0x00, 0x00, 0x00]);
            let len = payload.len() as u32;
            hdr.extend_from_slice(&len.to_be_bytes()[1..4]);
            hdr.push(msg_type);
            hdr.extend_from_slice(&stream_id.to_le_bytes());
            s.write_all(&hdr)?;
            first = false;
        } else {
            s.write_all(&dbl_basic_header(3, csid))?;
        }
        if take > 0 {
            s.write_all(&payload[offset..offset + take])?;
            offset += take;
        } else {
            break;
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct DblHeader {
    timestamp: u32,
    msg_len: usize,
    msg_type: u8,
    stream_id: u32,
    /// The header used the 0xFFFFFF escape: every Type 3 chunk of this chunk
    /// stream repeats the 4-byte extended timestamp (RTMP §5.3.1.3).
    ext: bool,
}

/// A conforming chunk reader: reads at the size the CLIENT announced, and
/// reads the extended timestamp wherever the spec puts it.
struct DblConn {
    in_chunk: usize,
    /// What the client announced with Set Chunk Size, if it did.
    announced: Option<usize>,
    last: std::collections::HashMap<u32, DblHeader>,
    partial: std::collections::HashMap<u32, (DblHeader, Vec<u8>)>,
}

struct DblMsg {
    msg_type: u8,
    timestamp: u32,
    payload: Vec<u8>,
}

fn invalid(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

impl DblConn {
    fn new() -> Self {
        Self {
            in_chunk: 128,
            announced: None,
            last: Default::default(),
            partial: Default::default(),
        }
    }

    fn read_n<const N: usize>(s: &mut TcpStream) -> std::io::Result<[u8; N]> {
        let mut b = [0u8; N];
        s.read_exact(&mut b)?;
        Ok(b)
    }

    fn read_u24(s: &mut TcpStream) -> std::io::Result<u32> {
        let b = Self::read_n::<3>(s)?;
        Ok(((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32)
    }

    /// Read chunks until one message completes. Set Chunk Size from the
    /// client is applied (it governs every later read) and not returned.
    fn next_message(&mut self, s: &mut TcpStream) -> std::io::Result<DblMsg> {
        loop {
            let fb = Self::read_n::<1>(s)?[0];
            let fmt = fb >> 6;
            let mut csid = (fb & 0x3F) as u32;
            if csid == 0 {
                csid = Self::read_n::<1>(s)?[0] as u32 + 64;
            } else if csid == 1 {
                let b = Self::read_n::<2>(s)?;
                csid = b[0] as u32 + b[1] as u32 * 256 + 64;
            }
            let prev = self.last.get(&csid).cloned();
            let hdr = match fmt {
                0 => {
                    let ts = Self::read_u24(s)?;
                    let len = Self::read_u24(s)? as usize;
                    let typ = Self::read_n::<1>(s)?[0];
                    let sid = u32::from_le_bytes(Self::read_n::<4>(s)?);
                    let ext = ts == 0xFF_FF_FF;
                    let ts = if ext {
                        u32::from_be_bytes(Self::read_n::<4>(s)?)
                    } else {
                        ts
                    };
                    DblHeader {
                        timestamp: ts,
                        msg_len: len,
                        msg_type: typ,
                        stream_id: sid,
                        ext,
                    }
                }
                1 | 2 => {
                    let delta = Self::read_u24(s)?;
                    let p = prev.ok_or_else(|| invalid("fmt=1/2 with no history"))?;
                    let (len, typ) = if fmt == 1 {
                        let len = Self::read_u24(s)? as usize;
                        (len, Self::read_n::<1>(s)?[0])
                    } else {
                        (p.msg_len, p.msg_type)
                    };
                    let ext = delta == 0xFF_FF_FF;
                    let ts = if ext {
                        u32::from_be_bytes(Self::read_n::<4>(s)?)
                    } else {
                        p.timestamp.wrapping_add(delta)
                    };
                    DblHeader {
                        timestamp: ts,
                        msg_len: len,
                        msg_type: typ,
                        stream_id: p.stream_id,
                        ext,
                    }
                }
                _ => {
                    let p = prev.ok_or_else(|| invalid("fmt=3 with no history"))?;
                    if p.ext {
                        // §5.3.1.3: the continuation repeats the extended
                        // timestamp. A writer that omits it hands us four
                        // payload bytes here instead.
                        let ext_ts = u32::from_be_bytes(Self::read_n::<4>(s)?);
                        if ext_ts != p.timestamp {
                            return Err(invalid(&format!(
                                "fmt=3 extended timestamp {ext_ts:#x} != header's {:#x}",
                                p.timestamp
                            )));
                        }
                    }
                    p
                }
            };
            self.last.insert(csid, hdr.clone());
            let (base, mut buf) = self
                .partial
                .remove(&csid)
                .unwrap_or((hdr.clone(), Vec::new()));
            let remaining = base.msg_len.saturating_sub(buf.len());
            let take = remaining.min(self.in_chunk);
            if take > 0 {
                let mut chunk = vec![0u8; take];
                s.read_exact(&mut chunk)?;
                buf.extend_from_slice(&chunk);
            }
            if buf.len() < base.msg_len {
                self.partial.insert(csid, (base, buf));
                continue;
            }
            if base.msg_type == 0x01 && buf.len() >= 4 {
                let size = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                if !(1..=0x7FFF_FFFF).contains(&size) {
                    return Err(invalid("absurd Set Chunk Size"));
                }
                self.in_chunk = size;
                self.announced = Some(size);
                continue;
            }
            return Ok(DblMsg {
                msg_type: base.msg_type,
                timestamp: base.timestamp,
                payload: buf,
            });
        }
    }
}

struct TestDouble {
    addr: SocketAddr,
    received: Arc<Mutex<Received>>,
    /// When set, the server stops reading (backpressure) without closing.
    stall: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<TcpStream>>>,
    listener_thread: Option<std::thread::JoinHandle<()>>,
}

impl TestDouble {
    fn start() -> Self {
        Self::start_with(0, DoubleOpts::default())
    }

    /// Bind an explicit port (loopback only). The reconnect test restarts on
    /// the SAME port the publisher redials.
    fn start_on(port: u16) -> Self {
        Self::start_with(port, DoubleOpts::default())
    }

    fn start_with(port: u16, opts: DoubleOpts) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("loopback bind must succeed");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener must succeed");
        let addr = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(Received::default()));
        let stall = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let conns: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let (received_c, stall_c, shutdown_c, conns_c) = (
            received.clone(),
            stall.clone(),
            shutdown.clone(),
            conns.clone(),
        );
        let thread = std::thread::spawn(move || {
            while !shutdown_c.load(Ordering::SeqCst) {
                let (stream, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                if let Ok(track) = stream.try_clone() {
                    conns_c.lock().unwrap().push(track);
                }
                let conn_no = {
                    let mut r = received_c.lock().unwrap();
                    r.connections += 1;
                    r.connections
                };
                let (rc, sc, dc) = (received_c.clone(), stall_c.clone(), shutdown_c.clone());
                std::thread::spawn(move || {
                    Self::serve(stream, conn_no, opts, &rc, &sc, &dc);
                });
            }
        });
        Self {
            addr,
            received,
            stall,
            shutdown,
            conns,
            listener_thread: Some(thread),
        }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Serve one connection: handshake, AMF0 dialog, chunked media.
    #[allow(clippy::too_many_lines)]
    fn serve(
        mut s: TcpStream,
        conn_no: u32,
        opts: DoubleOpts,
        received: &Arc<Mutex<Received>>,
        stall: &Arc<AtomicBool>,
        shutdown: &Arc<AtomicBool>,
    ) {
        s.set_nonblocking(false).ok();
        let mut c0 = [0u8; 1];
        let mut c1 = [0u8; 1536];
        if s.read_exact(&mut c0).is_err() || s.read_exact(&mut c1).is_err() || c0[0] != 3 {
            return;
        }
        let mut s1 = [0u8; 1536];
        s1[0] = 0x53;
        if s.write_all(&[3]).is_err() || s.write_all(&s1).is_err() || s.write_all(&c1).is_err() {
            return;
        }
        let mut c2 = [0u8; 1536];
        if s.read_exact(&mut c2).is_err() || c2 != s1 {
            return;
        }
        received.lock().unwrap().handshake_ok = true;

        let mut conn = DblConn::new();
        let mut out_chunk = 128usize;
        loop {
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            if stall.load(Ordering::SeqCst) {
                // Backpressure without closing: hold the connection, read
                // nothing — the client's socket buffers fill and its bounded
                // channel sheds by design.
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let msg = match conn.next_message(&mut s) {
                Ok(m) => m,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::InvalidData {
                        received
                            .lock()
                            .unwrap()
                            .parse_error
                            .get_or_insert(e.to_string());
                    }
                    return;
                }
            };
            if conn.announced.is_some() {
                received.lock().unwrap().client_chunk_size = conn.announced;
            }
            match msg.msg_type {
                0x14 => {
                    let Some((name, trans, rest)) = damf_command(&msg.payload) else {
                        continue;
                    };
                    match name.as_str() {
                        "connect" => {
                            for v in &rest {
                                if let DAmf::Object(props) = v {
                                    for (k, val) in props {
                                        if let (true, DAmf::String(a)) = (k == "app", val) {
                                            received.lock().unwrap().app = a.clone();
                                        }
                                    }
                                }
                            }
                            // Announce OUR outbound chunk size, as servers do.
                            if let Some(size) = opts.announce_chunk {
                                if dbl_write_msg(&mut s, out_chunk, 2, 0x01, 0, &size.to_be_bytes())
                                    .is_err()
                                {
                                    return;
                                }
                                out_chunk = size as usize;
                            }
                            let mut full = dbl_string("_result");
                            full.extend_from_slice(&dbl_number(trans));
                            full.extend_from_slice(&dbl_object(&[
                                ("fmsVer", dbl_string("FMS/3,0,1,123")),
                                ("capabilities", dbl_number(31.0)),
                            ]));
                            full.extend_from_slice(&dbl_object(&[
                                ("level", dbl_string("status")),
                                ("code", dbl_string("NetConnection.Connect.Success")),
                            ]));
                            if dbl_write_msg(&mut s, out_chunk, 3, 0x14, 0, &full).is_err() {
                                return;
                            }
                            received.lock().unwrap().connect_ok = true;
                        }
                        "createStream" => {
                            let mut payload = dbl_string("_result");
                            payload.extend_from_slice(&dbl_number(trans));
                            payload.extend_from_slice(&dbl_null());
                            payload.extend_from_slice(&dbl_number(1.0));
                            if dbl_write_msg(&mut s, out_chunk, 3, 0x14, 0, &payload).is_err() {
                                return;
                            }
                        }
                        "publish" => {
                            if let Some(DAmf::String(k)) =
                                rest.iter().find(|v| matches!(v, DAmf::String(_)))
                            {
                                received.lock().unwrap().key = k.clone();
                            }
                            let mut payload = dbl_string("onStatus");
                            payload.extend_from_slice(&dbl_number(0.0));
                            payload.extend_from_slice(&dbl_null());
                            payload.extend_from_slice(&dbl_object(&[
                                ("level", dbl_string("status")),
                                ("code", dbl_string("NetStream.Publish.Start")),
                            ]));
                            if dbl_write_msg(&mut s, out_chunk, 3, 0x14, 1, &payload).is_err() {
                                return;
                            }
                            received.lock().unwrap().publish_ok = true;
                            if let Some(ts) = opts.ping_after_publish {
                                let mut ping = 6u16.to_be_bytes().to_vec();
                                ping.extend_from_slice(&ts.to_be_bytes());
                                if dbl_write_msg(&mut s, out_chunk, 2, 0x04, 0, &ping).is_err() {
                                    return;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                0x04 if msg.payload.len() >= 6 && msg.payload[..2] == [0, 7] => {
                    let ts = u32::from_be_bytes([
                        msg.payload[2],
                        msg.payload[3],
                        msg.payload[4],
                        msg.payload[5],
                    ]);
                    received.lock().unwrap().pong = Some(ts);
                }
                0x09 => {
                    received
                        .lock()
                        .unwrap()
                        .video
                        .push((conn_no, msg.timestamp, msg.payload));
                }
                0x08 => {
                    received
                        .lock()
                        .unwrap()
                        .audio
                        .push((conn_no, msg.timestamp, msg.payload));
                }
                _ => {}
            }
        }
    }

    fn url(&self, app: &str, key: &str) -> String {
        format!("rtmp://{}/{app}/{key}", self.addr)
    }

    fn with<R>(&self, f: impl FnOnce(&Received) -> R) -> R {
        f(&self.received.lock().unwrap())
    }

    /// Kill the double mid-stream: force-close live connections (readers see
    /// EOF) and stop the listener. No FIN grace, no operator call.
    fn kill(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for c in self.conns.lock().unwrap().iter() {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
        if let Some(t) = self.listener_thread.take() {
            let _ = t.join();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Drop for TestDouble {
    fn drop(&mut self) {
        self.kill();
    }
}

// ===========================================================================
// Runner-independent: the transport and the stream thread, synthetic
// payloads, no encoder. Exercised on CI.
// ===========================================================================

/// Publish session against a conforming server, at each chunk size a real
/// server announces: the dialog lands (app, key), the client announces ITS
/// outbound chunk size and sends at it, and every media message — 6 KB
/// video, far over any single chunk — reassembles byte-exact at its media
/// timestamp.
///
/// Falsifies the chunk-size fix: PR #30's client adopted the server's
/// announced size for its own sends without announcing anything, so a
/// conforming server still reading at 128 misparsed every message over 128
/// bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publisher_dialog_and_media_reach_a_conforming_server() {
    let _serial = SERIAL.lock().await;
    for announce in [Some(4096u32), Some(65536), None] {
        let dbl = TestDouble::start_with(
            0,
            DoubleOpts {
                announce_chunk: announce,
                ..Default::default()
            },
        );
        let p = publisher(&dbl.url("live", "key-one"));
        assert!(
            wait_publisher_live(&p).await,
            "publisher must go Live (server announces {announce:?})"
        );
        admit(&p, true, video_sequence_header(), 0).await;
        admit(&p, false, audio_sequence_header(), 0).await;
        let mut sent_video = Vec::new();
        let mut sent_audio = Vec::new();
        for i in 0..30u32 {
            let v = patterned(&[0x27, 0x01, 0, 0, 0], 6000, i);
            let a = patterned(&[0xAF, 0x01], 300, 1000 + i);
            admit(&p, true, v.clone(), i * 33).await;
            admit(&p, false, a.clone(), i * 21).await;
            sent_video.push((i * 33, v));
            sent_audio.push((i * 21, a));
        }
        let arrived = poll_until(Duration::from_secs(5), || {
            dbl.with(|r| {
                r.parse_error.is_some()
                    || (r.video_media().len() >= 30 && r.audio_media().len() >= 30)
            })
        })
        .await;
        dbl.with(|r| {
            assert_eq!(
                r.parse_error, None,
                "a conforming server could not parse the client (server announces {announce:?})"
            );
            assert!(
                arrived,
                "all media must arrive (server announces {announce:?})"
            );
            assert!(r.handshake_ok && r.connect_ok && r.publish_ok);
            assert_eq!(r.app, "live");
            assert_eq!(r.key, "key-one");
            assert_eq!(
                r.client_chunk_size,
                Some(4096),
                "the client announces its own outbound chunk size, whatever the server says"
            );
            let got_v: Vec<_> = r
                .video_media()
                .iter()
                .map(|(_, t, p)| (*t, p.clone()))
                .collect();
            let got_a: Vec<_> = r
                .audio_media()
                .iter()
                .map(|(_, t, p)| (*t, p.clone()))
                .collect();
            assert!(
                got_v == sent_video,
                "video reassembled byte-exact at its media ts"
            );
            assert!(
                got_a == sent_audio,
                "audio reassembled byte-exact at its media ts"
            );
        });
        assert!(p.shutdown_and_wait(Duration::from_secs(2)).await);
    }
}

/// RTMP §5.3.1.3: once a message header carries the 0xFFFFFF escape, every
/// Type 3 continuation of that message repeats the 4-byte extended
/// timestamp. The double reads it where the spec puts it; a payload far over
/// the chunk size makes the continuations happen.
///
/// Falsifies the ext-ts fix: PR #30's writer omitted the repeat, so past
/// 0xFFFFFF ms (4 h 39 m 37 s on one connection) a conforming reader took
/// four payload bytes as the timestamp and desynchronised.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extended_timestamp_repeats_on_every_type3_chunk() {
    let _serial = SERIAL.lock().await;
    let dbl = TestDouble::start();
    let p = publisher(&dbl.url("live", "ext"));
    assert!(wait_publisher_live(&p).await);
    let base = 0x0100_0000u32; // past the 24-bit field: every header escapes
    admit(&p, true, video_sequence_header(), base).await;
    let sent: Vec<(u32, Vec<u8>)> = (0..4u32)
        .map(|i| {
            (
                base + 7 + i * 33,
                patterned(&[0x27, 0x01, 0, 0, 0], 10_000, i),
            )
        })
        .collect();
    for (ts, payload) in &sent {
        admit(&p, true, payload.clone(), *ts).await;
    }
    let arrived = poll_until(Duration::from_secs(5), || {
        dbl.with(|r| r.parse_error.is_some() || r.video_media().len() >= sent.len())
    })
    .await;
    dbl.with(|r| {
        assert_eq!(r.parse_error, None, "extended-timestamp chunks must parse");
        assert!(arrived, "every extended-timestamp message must arrive");
        let got: Vec<_> = r
            .video_media()
            .iter()
            .map(|(_, t, p)| (*t, p.clone()))
            .collect();
        assert!(
            got == sent,
            "payloads byte-exact at their extended timestamps"
        );
    });
    assert!(p.shutdown_and_wait(Duration::from_secs(2)).await);
}

/// The RTMP timestamp is the feeder's media time, not the socket's clock:
/// frames queued behind a stalled peer go out later but carry the timestamps
/// they were published with.
///
/// Falsifies the media-time fix: PR #30's transport stamped `t0.elapsed()`
/// at write time, so a stalled-then-drained backlog went out with
/// near-identical timestamps and every queueing hiccup became jitter.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn media_timestamps_are_the_feeders_not_the_sockets() {
    let _serial = SERIAL.lock().await;
    let dbl = TestDouble::start();
    let p = publisher(&dbl.url("live", "ts"));
    assert!(wait_publisher_live(&p).await);
    admit(&p, true, video_sequence_header(), 0).await;
    dbl.stall.store(true, Ordering::SeqCst);
    let sent: Vec<u32> = (0..20u32).map(|i| 5_000 + i * 33).collect();
    for ts in &sent {
        admit(&p, true, patterned(&[0x27, 0x01, 0, 0, 0], 500, *ts), *ts).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    dbl.stall.store(false, Ordering::SeqCst);
    assert!(
        poll_until(Duration::from_secs(5), || dbl
            .with(|r| r.video_media().len() >= 20))
        .await,
        "the backlog must drain after the stall"
    );
    let got: Vec<u32> = dbl.with(|r| r.video_media().iter().map(|(_, t, _)| *t).collect());
    assert_eq!(
        got, sent,
        "timestamps are media time, unchanged by the stall"
    );
    assert!(p.shutdown_and_wait(Duration::from_secs(2)).await);
}

/// nginx-rtmp pings a publisher (every 3 min by default) and drops one that
/// has not answered within `ping_timeout`. The live loop parses the server's
/// messages as messages and answers.
///
/// Falsifies the pong: PR #30's loop discarded whatever one raw read
/// returned and answered nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ping_requests_are_answered() {
    let _serial = SERIAL.lock().await;
    let dbl = TestDouble::start_with(
        0,
        DoubleOpts {
            ping_after_publish: Some(0x00C0_FFEE),
            ..Default::default()
        },
    );
    let p = publisher(&dbl.url("live", "ping"));
    assert!(wait_publisher_live(&p).await);
    assert!(
        poll_until(Duration::from_secs(3), || dbl.with(|r| r.pong.is_some())).await,
        "a PingRequest must be answered"
    );
    assert_eq!(
        dbl.with(|r| r.pong),
        Some(0x00C0_FFEE),
        "the pong echoes the ping's timestamp"
    );
    assert!(p.shutdown_and_wait(Duration::from_secs(2)).await);
}

/// Kill the server mid-stream: the publisher goes `Reconnecting`, redials on
/// its own when the server returns, and re-announces the codecs on the new
/// connection — at the stream's current media time, so the timeline
/// continues rather than restarting at zero. No operator action.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_kill_midstream_then_live_again_without_operator_action() {
    let _serial = SERIAL.lock().await;
    let mut dbl = TestDouble::start();
    let port = dbl.port();
    let p = publisher(&dbl.url("live", "re"));
    assert!(wait_publisher_live(&p).await);
    admit(&p, true, video_sequence_header(), 0).await;
    admit(&p, false, audio_sequence_header(), 0).await;
    for i in 0..10u32 {
        admit(&p, true, patterned(&[0x27, 0x01, 0, 0, 0], 800, i), i * 33).await;
    }
    assert!(
        poll_until(Duration::from_secs(3), || dbl
            .with(|r| r.video_media().len() >= 10))
        .await
    );

    let killed = Instant::now();
    dbl.kill();
    assert!(
        poll_until(Duration::from_secs(3), || {
            p.publisher_state() == PublisherState::Reconnecting
        })
        .await,
        "transport loss must read Reconnecting"
    );
    let noticed = killed.elapsed();
    // Publishing while the peer is gone never blocks and never errors out
    // of the stream: it sheds, counted.
    for i in 10..20u32 {
        let _ = p.try_publish_video(patterned(&[0x27, 0x01, 0, 0, 0], 800, i), i * 33);
    }

    let returned = Instant::now();
    let dbl2 = TestDouble::start_on(port);
    assert!(
        wait_publisher_live(&p).await,
        "the publisher must redial and go Live again on its own"
    );
    let redialed = returned.elapsed();
    for i in 20..25u32 {
        admit(&p, true, patterned(&[0x27, 0x01, 0, 0, 0], 800, i), i * 33).await;
    }
    assert!(
        poll_until(Duration::from_secs(3), || dbl2
            .with(|r| r.video_media().len() >= 5))
        .await
    );
    dbl2.with(|r| {
        assert_eq!(r.parse_error, None);
        let (_, seq_ts, first) = &r.video[0];
        assert!(
            Received::is_video_seq(first),
            "the redial re-announces the AVC sequence header first"
        );
        assert!(r.audio_seq_payload().is_some(), "and the AAC one");
        assert!(
            *seq_ts >= 19 * 33,
            "the replayed header continues the media timeline (ts {seq_ts}), never restarts it"
        );
        eprintln!(
            "RECONNECT: kill noticed (Reconnecting) in {noticed:?}; Live again {redialed:?} after \
             the ingest returned, no operator action; codecs re-announced first at media ts {seq_ts}"
        );
    });
    assert!(p.shutdown_and_wait(Duration::from_secs(2)).await);
}

/// Publishing never blocks the caller, whatever the peer does: behind a
/// stalled server every `try_publish` returns at once, and what does not fit
/// is shed and counted. (The stream thread is the caller in production; the
/// render loop never publishes at all.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publishes_never_block_under_backpressure() {
    let _serial = SERIAL.lock().await;
    let dbl = TestDouble::start();
    let p = publisher(&dbl.url("live", "bp"));
    assert!(wait_publisher_live(&p).await);
    dbl.stall.store(true, Ordering::SeqCst);
    let mut worst = Duration::ZERO;
    for i in 0..2000u32 {
        let payload = patterned(&[0x27, 0x01, 0, 0, 0], 50_000, i);
        let t = Instant::now();
        let _ = p.try_publish_video(payload, i * 33);
        worst = worst.max(t.elapsed());
    }
    eprintln!(
        "BACKPRESSURE: 2000 publishes behind a stalled peer, worst call {:?}, shed {}",
        worst,
        p.shed_frames()
    );
    // A publish that waited on the stalled peer would block indefinitely,
    // so any finite bound discriminates; 50 ms leaves room for scheduler
    // preemption on a shared 3-core CI runner.
    assert!(
        worst < Duration::from_millis(50),
        "a publish must never wait on the socket (worst {worst:?})"
    );
    assert!(p.shed_frames() > 0, "a full channel sheds, counted");
    assert_eq!(
        p.publisher_state(),
        PublisherState::Live,
        "backpressure is not a disconnect"
    );
    dbl.stall.store(false, Ordering::SeqCst);
    assert!(p.shutdown_and_wait(Duration::from_secs(2)).await);
}

/// `streamBufferMs` is the transport's actual buffered bytes through the
/// stream's own envelope bitrate: it grows while the peer stalls, drains to
/// zero when the peer reads, and divides by the envelope the stream was
/// opened at (the manifest's bitrates), not a constant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_buffer_ms_moves_with_load_not_a_constant() {
    let _serial = SERIAL.lock().await;
    let dbl = TestDouble::start();
    let envelope = 6_128_000u64; // 6000k video + 128k audio
    let p =
        spawn_publisher_with_envelope(parse_rtmp_url(&dbl.url("live", "buf")).unwrap(), envelope);
    assert!(wait_publisher_live(&p).await);
    assert_eq!(p.buffer_ms(), 0.0, "an idle live transport buffers nothing");
    dbl.stall.store(true, Ordering::SeqCst);
    for i in 0..64u32 {
        let _ = p.try_publish_video(patterned(&[0x27, 0x01, 0, 0, 0], 60_000, i), i * 33);
    }
    assert!(
        poll_until(Duration::from_secs(2), || p.buffer_ms() > 0.0).await,
        "a stalled peer must show buffered ms"
    );
    let (bytes, ms) = (p.buffered_bytes(), p.buffer_ms());
    assert!(
        (ms - bytes as f64 * 8000.0 / envelope as f64).abs() < 1e-6,
        "buffer_ms must be bytes through THIS stream's envelope ({bytes} B → {ms} ms)"
    );
    assert_ne!(
        envelope, ENVELOPE_BITRATE_BPS,
        "the test must not pass on the default"
    );
    dbl.stall.store(false, Ordering::SeqCst);
    assert!(
        poll_until(Duration::from_secs(5), || p.buffer_ms() == 0.0).await,
        "a drained transport reads 0 (read {} ms)",
        p.buffer_ms()
    );
    assert!(p.shutdown_and_wait(Duration::from_secs(2)).await);
}

/// The stream thread publishes the ENGINE's audio: the real audio driver
/// renders the master mix, pushes it into the stream's tap (the one
/// `stream.start` publishes in `state.stream_tap`), and the stream thread
/// drains it through AudioToolbox AAC onto the wire — sequence header from
/// the codec's own AudioSpecificConfig (48 kHz: `0x11 0x90`), packets at
/// sample-derived media time. No encoder needed: this runs on CI.
///
/// PR #30 shipped `publish_audio` with no production caller; the MediaMTX
/// "2 tracks" proof fed synthetic audio from inside the test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_thread_publishes_engine_audio() {
    let _serial = SERIAL.lock().await;
    if !nbe_engine::record::aac::is_available() {
        eprintln!("SKIP: no AudioToolbox AAC encoder on this machine");
        return;
    }
    let dbl = TestDouble::start();
    let (state, _handler, _) = harness();
    let audio = nbe_engine::audio_driver::spawn(state.clone(), 30);
    let session = StreamSession::open(
        dbl.url("live", "audio"),
        zero_copy_selection(),
        StreamParams::new(1920, 1080, 30),
        state.skipped_stream_frames.clone(),
    );
    // The one line of `stream.start` this test stands in for (it needs an
    // encoder CI does not have): publish the session's tap for the driver.
    *state.stream_tap.lock().unwrap() = Some(session.tap());
    let stats = session.stats();
    let got_audio = poll_until(Duration::from_secs(5), || {
        dbl.with(|r| r.audio_media().len() >= 40)
    })
    .await;
    *state.stream_tap.lock().unwrap() = None;
    audio.stop();
    let mut session = session;
    session.stop_and_close().await.expect("stream closes");
    assert!(
        stats.aac_ready.load(Ordering::SeqCst),
        "the thread opened AAC"
    );
    dbl.with(|r| {
        assert_eq!(r.parse_error, None);
        assert!(
            got_audio,
            "engine audio must reach the wire (got {})",
            r.audio_media().len()
        );
        assert_eq!(
            r.audio_seq_payload(),
            Some(&[0xAF, 0x00, 0x11, 0x90][..]),
            "the AAC sequence header is the codec's own ASC: LC, 48 kHz, stereo"
        );
        let ts: Vec<u32> = r.audio_media().iter().map(|(_, t, _)| *t).collect();
        let expected: Vec<u32> = (0..ts.len() as u64).map(audio_ts_ms).collect();
        assert_eq!(
            ts, expected,
            "audio timestamps are retained packets × 1024 samples at 48 kHz"
        );
        assert!(
            r.audio_media()
                .iter()
                .all(|(_, _, p)| p.starts_with(&[0xAF, 0x01]) && p.len() > 2),
            "every packet is a raw AAC FLV tag with a body"
        );
    });
}

/// The manifest's `audioBitrateKbps` reaches the codec: the stream thread
/// opens AAC at the stream's rate and AudioToolbox reports using it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manifest_audio_bitrate_reaches_the_aac_codec() {
    let _serial = SERIAL.lock().await;
    if !nbe_engine::record::aac::is_available() {
        eprintln!("SKIP: no AudioToolbox AAC encoder on this machine");
        return;
    }
    let dbl = TestDouble::start();
    let output: nbe_core::manifest::StreamOutput = serde_json::from_value(serde_json::json!({
        "url": dbl.url("live", "br"), "videoBitrateKbps": 6000, "audioBitrateKbps": 128
    }))
    .unwrap();
    let params = StreamParams::new(1920, 1080, 30).with_output(Some(&output));
    let mut session = StreamSession::open(
        dbl.url("live", "br"),
        zero_copy_selection(),
        params,
        Arc::new(AtomicU64::new(0)),
    );
    let stats = session.stats();
    assert!(
        poll_until(Duration::from_secs(3), || stats
            .aac_ready
            .load(Ordering::SeqCst))
        .await
    );
    assert_eq!(
        stats.aac_bit_rate.load(Ordering::SeqCst),
        128_000,
        "AudioToolbox must report the manifest's 128 kbps, not the 192 kbps default"
    );
    assert_eq!(session.params().envelope_bps(), 6_128_000);
    session.stop_and_close().await.expect("stream closes");
}

/// `stream.stop` against a transport that never came up (nothing listening)
/// still acks inside a hard bound: the stream thread exits, the mid-dial
/// publisher answers the stop, and the executor is never blocked. The stop
/// enters through the real directive; the session is opened directly
/// because `stream.start` needs an encoder CI does not have.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_stop_against_a_dead_transport_acks_inside_the_bound() {
    let _serial = SERIAL.lock().await;
    let dead_port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let url = format!("rtmp://127.0.0.1:{dead_port}/live/dead");
    let (state, handler, outgoing) = harness();
    let (_pkg, pkg_path) = write_package(&url);
    load_and_start(&handler, &pkg_path).await;
    let session = StreamSession::open(
        &url,
        zero_copy_selection(),
        StreamParams::new(1920, 1080, 30),
        state.skipped_stream_frames.clone(),
    );
    assert!(
        session.frame_sender().is_some(),
        "a publisher means a stream thread"
    );
    *state.stream_tap.lock().unwrap() = Some(session.tap());
    *state.stream_session.lock().unwrap() = Some(session);
    *state.stream_state.lock().unwrap() = StreamState::Live;
    tokio::time::sleep(Duration::from_millis(300)).await; // mid-redial

    let started = Instant::now();
    handler
        .apply(&directive("stream.stop", 3, serde_json::json!({})))
        .await
        .expect("stop against a dead transport must still succeed");
    let took = started.elapsed();
    eprintln!("BOUNDED STOP: stream.stop against a dead transport acked in {took:?}");
    assert!(
        took < Duration::from_millis(1500),
        "stop must be bounded (took {took:?})"
    );
    assert!(acked(&outgoing, 3), "a successful stop acks");
    assert_eq!(*state.stream_state.lock().unwrap(), StreamState::Idle);
    assert!(state.stream_session.lock().unwrap().is_none());
    assert!(
        state.stream_tap.lock().unwrap().is_none(),
        "every stop clears the stream tap"
    );
}

/// The §10.1 tick's `streamBufferMs` IS the live session's transport counter:
/// nonzero while the peer stalls, zero once drained, and equal to what the
/// session reports at each read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telemetry_tick_wires_the_live_session_counter() {
    let _serial = SERIAL.lock().await;
    let dbl = TestDouble::start();
    let (state, _handler, _) = harness();
    let session = StreamSession::open(
        dbl.url("live", "tick"),
        zero_copy_selection(),
        StreamParams::new(1920, 1080, 30),
        state.skipped_stream_frames.clone(),
    );
    *state.stream_session.lock().unwrap() = Some(session);
    *state.stream_state.lock().unwrap() = StreamState::Live;
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await
    );
    dbl.stall.store(true, Ordering::SeqCst);
    {
        let guard = state.stream_session.lock().unwrap();
        let p = guard
            .as_ref()
            .unwrap()
            .publisher()
            .expect("live session has a publisher");
        for i in 0..64u32 {
            let _ = p.try_publish_video(patterned(&[0x27, 0x01, 0, 0, 0], 60_000, i), 1000 + i);
        }
    }
    assert!(
        poll_until(Duration::from_secs(2), || tick_stream_buffer_ms(&state)
            > 0.0)
        .await
    );
    let session_ms = state
        .stream_session
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .stream_buffer_ms();
    let tick_ms = tick_stream_buffer_ms(&state);
    assert!(tick_ms > 0.0, "a stalled peer shows on the tick");
    assert!(
        (tick_ms - session_ms).abs() < 1e-9 || tick_ms <= session_ms,
        "the tick reads the session counter (tick {tick_ms}, session {session_ms})"
    );
    dbl.stall.store(false, Ordering::SeqCst);
    assert!(
        poll_until(Duration::from_secs(5), || tick_stream_buffer_ms(&state)
            == 0.0)
        .await,
        "drained reads 0 on the tick"
    );
    let mut s = state.stream_session.lock().unwrap().take().unwrap();
    s.stop_and_close().await.expect("stream closes");
}

#[tokio::test]
async fn error_kinds_carry_stable_tokens() {
    // RTMP transport failures are operator-facing E_NETWORK …
    let io = nbe_engine::record::rtmp::RtmpError::Io("dial refused".into());
    assert!(
        io.to_string().contains("E_NETWORK"),
        "RtmpError::Io must carry E_NETWORK, got: {io}"
    );
    let proto = nbe_engine::record::rtmp::RtmpError::Protocol("bad echo".into());
    assert!(
        proto.to_string().contains("E_NETWORK"),
        "RtmpError::Protocol must carry E_NETWORK, got: {proto}"
    );
    // … malformed endpoints refuse as E_BAD_PAYLOAD (never silent) …
    let parse =
        nbe_engine::record::rtmp::parse_rtmp_url("rtmp://host-only").expect_err("must refuse");
    assert!(
        parse.to_string().contains("E_BAD_PAYLOAD"),
        "parse refusal must carry E_BAD_PAYLOAD, got: {parse}"
    );
    // … the scheme matches case-insensitively, and anything else refuses …
    let upper = nbe_engine::record::rtmp::parse_rtmp_url("RTMP://127.0.0.1:1935/live/key")
        .expect("uppercase RTMP:// must parse");
    assert_eq!(upper.host, "127.0.0.1");
    assert_eq!(upper.port, 1935);
    assert_eq!(upper.app, "live");
    assert_eq!(upper.key, "key");
    let scheme = nbe_engine::record::rtmp::parse_rtmp_url("http://host/live/key")
        .expect_err("non-rtmp scheme must refuse");
    assert!(
        scheme.to_string().contains("E_BAD_PAYLOAD"),
        "scheme refusal must carry E_BAD_PAYLOAD, got: {scheme}"
    );
    let srt = nbe_engine::record::rtmp::parse_rtmp_url("srt://host/live")
        .expect_err("srt:// must refuse (SRT deferred by v0.4.5's narrowing)");
    assert!(
        srt.to_string().contains("E_BAD_PAYLOAD"),
        "srt refusal must carry E_BAD_PAYLOAD, got: {srt}"
    );
    // … and teardown failures are E_NETWORK (withheld-ack path).
    nbe_engine::record::stream::set_force_close_error(true);
    let mut s = StreamSession::open(
        "rtmp://example/live",
        zero_copy_selection(),
        StreamParams::new(1920, 1080, 30),
        Arc::new(AtomicU64::new(0)),
    );
    let err = s.stop_and_close().await.expect_err("armed seam must fail");
    assert!(
        err.to_string().contains("E_NETWORK"),
        "StreamError::Teardown must carry E_NETWORK, got: {err}"
    );
    nbe_engine::record::stream::set_force_close_error(false);
}

/// The static AAC header (synthetic transports only) is 48 kHz, like the
/// codec's own: PR #30's said 48 kHz in its comment and 44.1 kHz in its bits.
#[test]
fn static_audio_sequence_header_is_48k() {
    assert_eq!(audio_sequence_header(), vec![0xAF, 0x00, 0x11, 0x90]);
}

// ===========================================================================
// Hardware-gated: the live feed through the REAL loop. Skips loudly without
// an encoder or a zero-copy chain (rule 8: ran − skipped = exercised).
// ===========================================================================

/// Assert a video timeline advances at `rate`: strictly increasing, every
/// step a whole number of frame periods (a shed frame leaves a gap, never a
/// compressed step).
fn assert_timeline_at(ts: &[u32], rate: u32) {
    assert!(ts.len() >= 2, "need a timeline, got {ts:?}");
    let period = 1000.0 / rate as f64;
    for w in ts.windows(2) {
        let delta = w[1] as f64 - w[0] as f64;
        let k = (delta / period).round();
        assert!(
            k >= 1.0 && (delta - k * period).abs() <= 1.0,
            "timestamps {} → {} are not a whole number of {rate} fps periods ({period:.2} ms)",
            w[0],
            w[1]
        );
    }
}

/// The whole pipeline on the production loop: `stream.start` → the loop's
/// tick hands surfaces to the stream thread → VideoToolbox → FLV → RTMP, and
/// the audio driver's master mix → the stream tap → AAC → RTMP. The double
/// receives the encoder's own sequence header first, a keyframe, frames at
/// the show's rate, and the engine's audio; the loop's stream work stays
/// send-shaped (the encoder never runs on it).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_loop_publishes_encoded_video_and_engine_audio() {
    let _serial = SERIAL.lock().await;
    let (state, handler, _) = harness();
    let Some(mut render) = live_rig_or_skip(&state).await else {
        return;
    };
    let audio = nbe_engine::audio_driver::spawn(state.clone(), 30);
    let dbl = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "loop"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open on a machine with encoder + chain");
    let reports = run_frames(&mut render, &state, 30, 75).await;
    let arrived = poll_until(Duration::from_secs(5), || {
        dbl.with(|r| r.video_media().len() >= 45 && r.audio_media().len() >= 60)
    })
    .await;
    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("stream.stop must succeed");
    audio.stop();

    let worst_stream = reports.iter().map(|r| r.stream).max().unwrap();
    let sent = reports.iter().filter(|r| r.stream_sent).count();
    eprintln!(
        "LIVE LOOP: {} ticks, {sent} surfaces handed off, worst stream share of a tick {worst_stream:?}, skipped_stream_frames {}",
        reports.len(),
        state.skipped_stream_frames.load(Ordering::SeqCst)
    );
    dbl.with(|r| {
        assert_eq!(r.parse_error, None);
        assert!(
            arrived,
            "video {} / audio {} must arrive",
            r.video_media().len(),
            r.audio_media().len()
        );
        let first = r.first_video_on(1).expect("video arrived");
        assert!(
            Received::is_video_seq(first),
            "the encoder's sequence header goes first"
        );
        assert!(
            r.video_media()
                .iter()
                .any(|(_, _, p)| p.starts_with(&[0x17, 0x01])),
            "a keyframe NALU arrives"
        );
        let ts: Vec<u32> = r.video_media().iter().map(|(_, t, _)| *t).collect();
        assert_timeline_at(&ts, 30);
        assert_eq!(r.audio_seq_payload(), Some(&[0xAF, 0x00, 0x11, 0x90][..]));
    });
    assert!(
        worst_stream < Duration::from_millis(5),
        "the loop's stream work is a surface loan and a try_send, never an encode ({worst_stream:?})"
    );
}

/// The show's frame rate is the stream's: a 60 fps show streams at 60 fps
/// (timestamps step 16–17 ms), and the manifest's bitrates are the stream's.
/// PR #30 opened the encoder at a hardcoded 30 fps / 8 Mbps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sixty_fps_show_streams_at_sixty_with_its_bitrates() {
    let _serial = SERIAL.lock().await;
    let (state, handler, _) = harness_at(60);
    let Some(mut render) = live_rig_or_skip(&state).await else {
        return;
    };
    let dbl = TestDouble::start();
    let (_pkg, pkg_path) = write_package_with(
        &dbl.url("live", "sixty"),
        60,
        serde_json::json!({ "videoBitrateKbps": 6000, "audioBitrateKbps": 128 }),
    );
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    let params = state
        .stream_session
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .params();
    assert_eq!(params.fps, 60, "the stream runs at the show's rate");
    assert_eq!(
        params.video_bitrate_bps, 6_000_000,
        "videoBitrateKbps reaches the encoder"
    );
    assert_eq!(
        params.audio_bitrate_bps, 128_000,
        "audioBitrateKbps reaches the codec"
    );
    let _ = run_frames(&mut render, &state, 60, 120).await;
    let arrived = poll_until(Duration::from_secs(5), || {
        dbl.with(|r| r.video_media().len() >= 60)
    })
    .await;
    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("stream.stop must succeed");
    dbl.with(|r| {
        assert!(
            arrived,
            "60 fps frames must arrive (got {})",
            r.video_media().len()
        );
        let ts: Vec<u32> = r.video_media().iter().map(|(_, t, _)| *t).collect();
        assert_timeline_at(&ts, 60);
        let one_step = ts.windows(2).filter(|w| w[1] - w[0] <= 17).count();
        assert!(
            one_step * 2 > ts.len(),
            "most steps are one 60 fps period (16–17 ms): {one_step} of {}",
            ts.len() - 1
        );
    });
}

/// Two streams in a row: the second opens with its own sequence header —
/// per-stream state lives and dies with the stream thread, by construction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_stream_emits_seq_header_first() {
    let _serial = SERIAL.lock().await;
    let (state, handler, _) = harness();
    let Some(mut render) = live_rig_or_skip(&state).await else {
        return;
    };
    let dbl1 = TestDouble::start();
    let dbl2 = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl1.url("live", "one"));
    load_and_start(&handler, &pkg_path).await;
    for (sv, dbl) in [(3u64, &dbl1), (5u64, &dbl2)] {
        handler
            .apply(&directive(
                "stream.start",
                sv,
                serde_json::json!({ "url": dbl.url("live", "k") }),
            ))
            .await
            .expect("stream.start must open");
        let _ = run_frames(&mut render, &state, 30, 30).await;
        assert!(
            poll_until(Duration::from_secs(5), || dbl
                .with(|r| r.video_media().len() >= 10))
            .await
        );
        handler
            .apply(&directive("stream.stop", sv + 1, serde_json::json!({})))
            .await
            .expect("stream.stop must succeed");
        dbl.with(|r| {
            let first = r.first_video_on(1).expect("video arrived");
            assert!(
                Received::is_video_seq(first),
                "stream {sv}: the first video payload is the AVC sequence header"
            );
        });
    }
}

/// Both outputs live on the zero-copy path: one composite, two holders. The
/// stream takes the record loan's surface (its own pool is never touched),
/// and a stream never costs record a frame (G1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_live_stream_shares_the_record_composite() {
    let _serial = SERIAL.lock().await;
    let (state, handler, _) = harness();
    let Some(mut render) = live_rig_or_skip(&state).await else {
        return;
    };
    let dbl = TestDouble::start();
    let (_pkg, pkg_path, _rec) =
        write_record_package(&dbl.url("live", "both"), serde_json::json!({}));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("record.start", 3, serde_json::json!({})))
        .await
        .expect("record.start must open");
    handler
        .apply(&directive("stream.start", 4, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Recording);
    let own_pool = state
        .stream_session
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .surface_pool()
        .expect("the stream owns a pool");
    let reports = run_frames(&mut render, &state, 30, 60).await;
    let arrived = poll_until(Duration::from_secs(5), || {
        dbl.with(|r| r.video_media().len() >= 30)
    })
    .await;
    let record_skips = state.skipped_record_frames.load(Ordering::SeqCst);
    let stream_skips = state.skipped_stream_frames.load(Ordering::SeqCst);
    let own_free = own_pool.free();
    handler
        .apply(&directive("stream.stop", 5, serde_json::json!({})))
        .await
        .expect("stream.stop must succeed");
    handler
        .apply(&directive("record.stop", 6, serde_json::json!({})))
        .await
        .expect("record.stop must succeed");
    eprintln!(
        "BOTH LIVE: {} ticks, record skips {record_skips}, stream skips {stream_skips}, video {}",
        reports.len(),
        dbl.with(|r| r.video_media().len())
    );
    assert!(
        arrived,
        "the stream receives frames from the shared composite"
    );
    assert_eq!(
        own_free,
        own_pool.len(),
        "the stream's own pool is untouched while it shares"
    );
    assert_eq!(record_skips, 0, "a live stream never costs record a frame");
}

/// A CPU-readback take beside a live stream: the take draws to the built-in
/// target and reads back; the stream draws into its own pool's surface. Both
/// progress.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cpu_record_beside_live_stream_still_feeds() {
    let _serial = SERIAL.lock().await;
    let (state, handler, _) = harness();
    let Some(mut render) = live_rig_or_skip(&state).await else {
        return;
    };
    let dbl = TestDouble::start();
    let (_pkg, pkg_path, _rec) = write_record_package(
        &dbl.url("live", "cpu"),
        serde_json::json!({ "tapPath": "cpuReadback" }),
    );
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("record.start", 3, serde_json::json!({})))
        .await
        .expect("record.start must open");
    handler
        .apply(&directive("stream.start", 4, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    let reports = run_frames(&mut render, &state, 30, 45).await;
    let arrived = poll_until(Duration::from_secs(5), || {
        dbl.with(|r| r.video_media().len() >= 20)
    })
    .await;
    handler
        .apply(&directive("stream.stop", 5, serde_json::json!({})))
        .await
        .expect("stream.stop must succeed");
    handler
        .apply(&directive("record.stop", 6, serde_json::json!({})))
        .await
        .expect("record.stop must succeed");
    assert!(
        arrived,
        "the stream feeds from its own pool beside a CPU take"
    );
    assert!(
        reports.iter().filter(|r| r.stream_sent).count() >= 20,
        "surfaces were handed off"
    );
}

/// Survival (§9.5): kill the ingest mid-stream while the real loop runs. The
/// View never notices (no dropped frames), the loop's stream work stays a
/// send, the transport reads Reconnecting, and when the ingest returns the
/// stream resumes on its own — re-announcing its codecs first.
///
/// Since v0.4.6 the redial is also ON THE WIRE: the tick's
/// `streamTransportState` reads `live` → `reconnecting` → `live` → `closed`
/// across the kill, the return and the stop, while the engine's `streamState`
/// stays Live throughout the redial.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transport_death_leaves_the_loop_untouched() {
    let _serial = SERIAL.lock().await;
    let (state, handler, _) = harness();
    let Some(mut render) = live_rig_or_skip(&state).await else {
        return;
    };
    let mut dbl = TestDouble::start();
    let port = dbl.port();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "survive"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    let _ = run_frames(&mut render, &state, 30, 30).await;
    assert!(
        poll_until(Duration::from_secs(5), || dbl
            .with(|r| r.video_media().len() >= 10))
        .await
    );
    assert_eq!(
        tick_transport_state(&state),
        "live",
        "media is flowing: the tick reads live"
    );

    let before = dropped(&state);
    dbl.kill();
    let during = run_frames(&mut render, &state, 30, 60).await;
    let after = dropped(&state);
    assert_eq!(
        publisher_state_of(&state),
        PublisherState::Reconnecting,
        "the transport reads Reconnecting"
    );
    assert_eq!(
        tick_transport_state(&state),
        "reconnecting",
        "the redial is visible on the wire (v0.4.6), not only inside the engine"
    );
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Live,
        "the stream stays Live"
    );
    let worst = during.iter().map(|r| r.stream).max().unwrap();
    eprintln!("SURVIVAL: dropped {before} → {after} across the kill, worst stream share {worst:?}");
    assert_eq!(
        after - before,
        0,
        "a dead transport never drops a View frame"
    );
    assert!(
        worst < Duration::from_millis(5),
        "the loop never waits on the transport ({worst:?})"
    );

    let dbl2 = TestDouble::start_on(port);
    let _ = run_frames(&mut render, &state, 30, 60).await;
    assert!(
        poll_until(Duration::from_secs(5), || dbl2
            .with(|r| r.video_media().len() >= 10))
        .await,
        "the stream resumes on the returned ingest without operator action"
    );
    assert_eq!(
        tick_transport_state(&state),
        "live",
        "the resumed transport reads live again"
    );
    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("stream.stop must succeed");
    assert_eq!(
        tick_transport_state(&state),
        "closed",
        "a stopped stream's transport reads closed"
    );
    dbl2.with(|r| {
        let first = r.first_video_on(1).expect("video arrived");
        assert!(
            Received::is_video_seq(first),
            "the redial re-announces the codec first"
        );
    });
}

// ---------------------------------------------------------------------------
// REAL RTMP interop against MediaMTX, with the ENGINE's pipeline.
//
// Downloads nothing (the binary arrives out of band in /tmp only — never
// committed): probes `/tmp/mediamtx-test/mediamtx` then `/tmp/mediamtx` and
// skips loudly when absent. The server's own log is the proof.
// ---------------------------------------------------------------------------

fn mediamtx_binary() -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    ["/tmp/mediamtx-test/mediamtx", "/tmp/mediamtx"]
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| {
            std::fs::metadata(p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

fn tcp_open(addr: &str) -> bool {
    std::net::TcpStream::connect(addr).is_ok()
}

/// The engine publishes to a REAL MediaMTX: `stream.start`, the production
/// loop, the stream thread's VideoToolbox H.264 and AudioToolbox AAC of the
/// audio driver's master mix. MediaMTX's own log line
/// `2 tracks (H264, MPEG-4 Audio)` must come from that pipeline — PR #30's
/// version fed hand-written audio from inside the test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mediamtx_proof_engine_pipeline_publishes_h264_and_aac() {
    let _serial = SERIAL.lock().await;
    let Some(bin) = mediamtx_binary() else {
        eprintln!(
            "SKIP: MediaMTX proof needs the out-of-band binary at /tmp/mediamtx-test/mediamtx \
             (or /tmp/mediamtx), never committed. The in-process double is the CI path."
        );
        return;
    };
    let (state, handler, _) = harness();
    let Some(mut render) = live_rig_or_skip(&state).await else {
        return;
    };
    let workdir = tempfile::Builder::new()
        .prefix("nbe-mediamtx-proof")
        .tempdir_in("/tmp")
        .expect("proof workdir in /tmp must succeed");
    let cfg = workdir.path().join("mediamtx.yml");
    std::fs::write(
        &cfg,
        "logLevel: debug\napi: true\napiAddress: 127.0.0.1:19998\nrtmpAddress: 127.0.0.1:1935\n\
         paths:\n  live/nbe-proof:\n",
    )
    .unwrap();
    let log_path = workdir.path().join("server.log");
    let log_file = std::fs::File::create(&log_path).unwrap();
    let mut child = std::process::Command::new(&bin)
        .arg(&cfg)
        .current_dir(workdir.path())
        .stdout(std::process::Stdio::from(log_file.try_clone().unwrap()))
        .stderr(std::process::Stdio::from(log_file))
        .spawn()
        .expect("mediamtx must spawn");
    let kill = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    let mut ready = false;
    for _ in 0..100 {
        if tcp_open("127.0.0.1:1935") && tcp_open("127.0.0.1:19998") {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        kill(&mut child);
        panic!("MediaMTX did not open RTMP/API listeners; server log:\n{log}");
    }

    let audio = nbe_engine::audio_driver::spawn(state.clone(), 30);
    let (_pkg, pkg_path) = write_package("rtmp://127.0.0.1:1935/live/nbe-proof");
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start at MediaMTX must open");
    // ~4 s of show: gortmplib settles tracks after a ~2 s timestamp span.
    let _ = run_frames(&mut render, &state, 30, 120).await;

    let mut tracks_line = None;
    let mut publishing_line = None;
    for _ in 0..50 {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        for line in log.lines() {
            if line.contains("2 tracks (H264, MPEG-4 Audio)") {
                tracks_line = Some(line.to_string());
            }
            if line.contains("is publishing to path 'live/nbe-proof'") {
                publishing_line = Some(line.to_string());
            }
        }
        if tracks_line.is_some() && publishing_line.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await;
    audio.stop();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    kill(&mut child);
    // Complaints about OUR connection or path (MediaMTX also warns at
    // startup about unrelated listeners, e.g. generating a MoQ certificate).
    let problems: Vec<&str> = log
        .lines()
        .filter(|l| l.contains(" ERR ") || l.contains(" WAR "))
        .filter(|l| l.contains("[RTMP]") || l.contains("nbe-proof"))
        .collect();
    match (tracks_line, publishing_line) {
        (Some(t), Some(p)) => {
            eprintln!("MEDIAMTX-PROOF {t}");
            eprintln!("MEDIAMTX-PROOF {p}");
            for l in &problems {
                eprintln!("MEDIAMTX-PROOF server complaint: {l}");
            }
        }
        _ => panic!("MediaMTX shows no incoming H.264+AAC stream; server log:\n{log}"),
    }
    assert!(
        problems.is_empty(),
        "MediaMTX must not complain about the engine's stream:\n{}",
        problems.join("\n")
    );
}

// ---------------------------------------------------------------------------
// MEASUREMENT, not a gate (`--ignored`): the loop's timed region with and
// without live outputs, and `stream.start` / `stream.stop` on the directive
// path. Thresholds belong to the soak (quiescent reference machine); this
// prints the numbers docs/09-measurements.md records, with the load.
// ---------------------------------------------------------------------------

fn load_1m() -> String {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .expect("sysctl must run");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// This process's cumulative CPU time (user + system), from `ps`.
fn cpu_seconds() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps must run");
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // [[hh:]mm:]ss.xx
    t.split(':').fold(0.0, |acc, part| {
        acc * 60.0 + part.parse::<f64>().unwrap_or(0.0)
    })
}

fn summarize(label: &str, xs: &[Duration], budget: Duration) -> String {
    let mut ms: Vec<f64> = xs.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = ms.len();
    let mean = ms.iter().sum::<f64>() / n as f64;
    let pct = |p: f64| ms[((n as f64 * p).ceil() as usize).clamp(1, n) - 1];
    let over = xs.iter().filter(|d| **d > budget).count();
    format!(
        "{label}: n={n} mean={mean:.3} p50={:.3} p95={:.3} max={:.3} over_budget={over}",
        pct(0.50),
        pct(0.95),
        ms[n - 1]
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "measurement: run with --ignored --nocapture on a quiescent machine"]
async fn measure_loop_timed_region_by_output() {
    let _serial = SERIAL.lock().await;
    let (state, handler, _) = harness();
    let Some(mut render) = live_rig_or_skip(&state).await else {
        return;
    };
    let audio = nbe_engine::audio_driver::spawn(state.clone(), 30);
    let dbl = TestDouble::start();
    let (_pkg, pkg_path, _rec) =
        write_record_package(&dbl.url("live", "measure"), serde_json::json!({}));
    load_and_start(&handler, &pkg_path).await;
    let budget = Duration::from_secs_f64(1.0 / 30.0);
    let mut sv = 10u64;
    let mut apply = |cmd: &'static str| {
        sv += 1;
        (directive(cmd, sv, serde_json::json!({})), cmd)
    };
    // Warm the loop (first-frame shader/pipeline costs are not the outputs').
    let _ = run_frames(&mut render, &state, 30, 60).await;
    for config in ["none", "record", "stream", "both"] {
        if matches!(config, "record" | "both") {
            let (d, _) = apply("record.start");
            handler.apply(&d).await.expect("record.start");
        }
        if matches!(config, "stream" | "both") {
            let (d, _) = apply("stream.start");
            let t = Instant::now();
            handler.apply(&d).await.expect("stream.start");
            println!(
                "MEASURE [{config}] stream.start directive: {:?}",
                t.elapsed()
            );
        }
        let skipped_r0 = state.skipped_record_frames.load(Ordering::SeqCst);
        let skipped_s0 = state.skipped_stream_frames.load(Ordering::SeqCst);
        let dropped0 = dropped(&state);
        let load_before = load_1m();
        let cpu0 = cpu_seconds();
        let wall0 = Instant::now();
        let reports = run_frames(&mut render, &state, 30, 300).await;
        let wall = wall0.elapsed().as_secs_f64();
        let cpu = cpu_seconds() - cpu0;
        let load_after = load_1m();
        let totals: Vec<Duration> = reports.iter().map(|r| r.total).collect();
        let streams: Vec<Duration> = reports.iter().map(|r| r.stream).collect();
        let records: Vec<Duration> = reports.iter().map(|r| r.record).collect();
        println!("MEASURE [{config}] load before {load_before} after {load_after}");
        println!(
            "MEASURE [{config}] {}",
            summarize("tick total ms", &totals, budget)
        );
        println!(
            "MEASURE [{config}] {}",
            summarize("stream share ms", &streams, budget)
        );
        println!(
            "MEASURE [{config}] {}",
            summarize("record share ms", &records, budget)
        );
        println!(
            "MEASURE [{config}] process CPU {:.1}% of one core over {wall:.1} s; record skips {}; stream skips {}; View drops {}",
            cpu / wall * 100.0,
            state.skipped_record_frames.load(Ordering::SeqCst) - skipped_r0,
            state.skipped_stream_frames.load(Ordering::SeqCst) - skipped_s0,
            dropped(&state) - dropped0
        );
        if let Some(s) = state.stream_session.lock().unwrap().as_ref() {
            let st = s.stats();
            let frames = st.video_frames_encoded.load(Ordering::SeqCst).max(1);
            println!(
                "MEASURE [{config}] stream thread: encoder open {} µs (on the thread), {} frames encoded, mean encode call {:.3} ms (on the thread)",
                st.encoder_open_us.load(Ordering::SeqCst),
                frames,
                st.encode_us_total.load(Ordering::SeqCst) as f64 / frames as f64 / 1000.0
            );
        }
        if matches!(config, "stream" | "both") {
            let (d, _) = apply("stream.stop");
            let t = Instant::now();
            handler.apply(&d).await.expect("stream.stop");
            println!(
                "MEASURE [{config}] stream.stop directive: {:?}",
                t.elapsed()
            );
        }
        if matches!(config, "record" | "both") {
            let (d, _) = apply("record.stop");
            let _ = handler.apply(&d).await;
        }
    }
    // stream.start on the directive path, ten cycles.
    let mut starts = Vec::new();
    for _ in 0..10 {
        let (d, _) = apply("stream.start");
        let t = Instant::now();
        handler.apply(&d).await.expect("stream.start");
        starts.push(t.elapsed());
        let (d, _) = apply("stream.stop");
        handler.apply(&d).await.expect("stream.stop");
    }
    println!(
        "MEASURE {}",
        summarize("stream.start directive ms (10 cycles)", &starts, budget)
    );
    audio.stop();
}
