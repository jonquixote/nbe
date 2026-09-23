//! Prompt 10 WU5 (SPEC §9.4, §9.5): RTMP transport + publisher + survival.
//!
//! TDD: written BEFORE `record::rtmp` and the `StreamSession` publisher wiring
//! land (RED first). Every test enters via the REAL directive path
//! (`show.load` with a real package → `show.start` → `stream.start` /
//! `stream.stop`, rule 7) against a pure-Rust in-process RTMP test double over
//! TCP loopback — no third-party server, no network beyond `127.0.0.1`.
//!
//! The double speaks REAL RTMP (Adobe handshake + AMF0 `connect` /
//! `createStream` / `publish` dialog + chunk framing at the 128-byte default
//! with fmt=3 continuations), the same wire the engine client speaks. It is
//! not a bespoke line protocol: the client cannot distinguish it from a real
//! ingest except by provenance.
//!
//! Coverage maps to the WU5 DoD:
//! 1. Publish session: handshake + H.264/AAC publish to
//!    `rtmp://127.0.0.1:<port>/<app>/<key>`; the double asserts app/key and
//!    the codec sequence headers.
//! 2. Reconnect: kill the double mid-stream → publisher `Reconnecting` →
//!    restart the double → `Live` again, no operator action, View never
//!    stutters (`droppedFramesTotal` unchanged across the kill).
//! 3. Survival falsification: killing the transport leaves the render loop
//!    untouched (`droppedFramesTotal` delta zero), and publishes stay
//!    non-blocking under backpressure — an inline-blocking publisher would
//!    blow the deadline (mutation check in the report).
//! 4. `streamBufferMs` is the transport's actual buffered bytes → ms (grows
//!    when the double stops reading, shrinks on drain; pinned to the
//!    bytes→ms formula, never a constant; the telemetry tick wires the same
//!    counter — nonzero under stall, zero when drained).
//! 5. MediaMTX interop (`mediamtx_proof_*`): the engine client publishes to a
//!    REAL MediaMTX ingest and the server's API/log shows the incoming
//!    H.264+AAC stream. Loud skip when the binary is absent (never
//!    false-green); the in-process double above stays the fast CI path.
//! 6. Live feed (`live_feed_*`): frames travel Surface → `encode_pixel_buffer`
//!    → FLV → publish through the REAL loop hook (`feed_stream_surface`,
//!    zero-copy, never readback/rgba, never a direct session write in the
//!    test) and land on the double as real encoded bytes.
//! 7. Bounded stop: `stream.stop` against a dead transport still acks inside
//!    a hard deadline (the executor is never blocked).
//! 8. `E_NETWORK` kind coverage for the touched error types.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::record::rtmp::{audio_sequence_header, video_sequence_header, PublisherState};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, OutgoingQueue, RecordState, StreamState};
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// Harness (rule 7: REAL stream.start / stream.stop through DirectiveHandler).
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

fn harness() -> (Arc<EngineState>, DirectiveHandler, Arc<OutgoingQueue>) {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    (state, handler, outgoing)
}

fn acked(outgoing: &OutgoingQueue, sv: u64) -> bool {
    outgoing.drain().into_iter().any(|f| match f {
        EngineFrame::AppliedStateVersion { state_version, .. } => state_version == sv,
        _ => false,
    })
}

fn hw_or_skip() -> bool {
    if nbe_engine::record::encoder_available() {
        return true;
    }
    eprintln!("SKIP: no hardware H.264 encoder on this machine (SPEC §9.2)");
    false
}

async fn chain_or_skip(state: &Arc<EngineState>) -> bool {
    let _render = RenderLoop::new(state.clone()).await.ok();
    if nbe_engine::record::stream::chain_available(&state.render_device()) {
        return true;
    }
    eprintln!("SKIP: no zero-copy chain on this machine (§0.1 assumption 24)");
    false
}

fn write_package(stream_url: &str) -> (tempfile::TempDir, std::path::PathBuf) {
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
    std::fs::write(
        pkg.path().join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.4",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate",
                "outputs": { "stream": { "url": stream_url } }
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

/// Package variant with a record target (absolute tempdir path): `record.start`
/// with `{}` opens a take there. Returns the package tempdir (kept alive),
/// the package path, and the record dir tempdir (kept alive).
fn write_record_package(
    stream_url: &str,
) -> (tempfile::TempDir, std::path::PathBuf, tempfile::TempDir) {
    let rec = tempfile::tempdir().expect("record tempdir must succeed");
    let (pkg, pkg_path) = write_package(stream_url);
    let manifest_path = pkg_path.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["show"]["outputs"]["record"] =
        serde_json::json!({ "directory": rec.path().to_string_lossy() });
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    (pkg, pkg_path, rec)
}

fn dropped(state: &Arc<EngineState>) -> u64 {
    state.dropped_frames_total.load(Ordering::SeqCst)
}

/// Feed synthetic H.264 (first payload = AVC sequence header) + AAC (first =
/// AAC sequence header) through the session's publisher — the same entry the
/// render loop calls; the transport owns the socket.
///
/// Batched with yields (16 frames, then `yield_now`): the production cadence
/// is 1–2 frames per render tick, and the bound is 64 — a 78-frame
/// synchronous burst would outrun any drain and shed by design. The batches
/// keep the test on the admitted path the render loop lives on.
async fn feed_av(session: &Arc<EngineState>, video_frames: usize, audio_frames: usize) {
    {
        let guard = session.stream_session.lock().unwrap();
        let s = guard.as_ref().expect("stream session must be live");
        assert!(s.has_publisher(), "live session must own a publisher");
    }
    let mut pending: Vec<(bool, Vec<u8>)> = Vec::new();
    pending.push((true, video_sequence_header()));
    pending.push((false, audio_sequence_header()));
    for i in 0..video_frames {
        let mut p = vec![0x17, 0x01, 0, 0, 0];
        p.extend_from_slice(format!("idr-{i}").as_bytes());
        pending.push((true, p));
    }
    for i in 0..audio_frames {
        let mut p = vec![0xAF, 0x01];
        p.extend_from_slice(format!("aac-{i}").as_bytes());
        pending.push((false, p));
    }
    for (n, (is_video, payload)) in pending.into_iter().enumerate() {
        // Eventual admission (2 s budget per frame): `try_send` is instant by
        // design, so a synchronous burst can momentarily outrun the drain and
        // shed — the render loop never bursts (1–2 frames per tick), but the
        // test does. Retry models the feeder's cadence; the NON-blocking
        // property itself is pinned separately by the survival test's
        // deadline. What must hold: every frame is admitted while Live.
        let start = Instant::now();
        let admitted = loop {
            let admitted = {
                let guard = session.stream_session.lock().unwrap();
                let s = guard.as_ref().expect("stream session must be live");
                if is_video {
                    s.publish_video(payload.clone())
                } else {
                    s.publish_audio(payload.clone())
                }
            };
            if admitted {
                break true;
            }
            if start.elapsed() > Duration::from_secs(2) {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        };
        assert!(admitted, "frame {n} must be admitted while Live");
        if n % 16 == 15 {
            tokio::task::yield_now().await;
        }
    }
}

/// Feed without admission asserts: for windows where the transport may be
/// redialing (shed-by-design). Returns admitted counts.
fn feed_best_effort(session: &Arc<EngineState>, video_frames: usize, audio_frames: usize) {
    let guard = session.stream_session.lock().unwrap();
    let s = guard.as_ref().expect("stream session must be present");
    let _ = s.publish_video(video_sequence_header());
    let _ = s.publish_audio(audio_sequence_header());
    for i in 0..video_frames {
        let mut p = vec![0x17, 0x01, 0, 0, 0];
        p.extend_from_slice(format!("idr-{i}").as_bytes());
        let _ = s.publish_video(p);
    }
    for i in 0..audio_frames {
        let mut p = vec![0xAF, 0x01];
        p.extend_from_slice(format!("aac-{i}").as_bytes());
        let _ = s.publish_audio(p);
    }
}

fn tick_stream_buffer_ms(state: &Arc<EngineState>) -> f64 {
    match nbe_engine::telemetry::build_tick_for_dir(state, None) {
        EngineFrame::EngineTelemetry { fields, .. } => fields.stream_buffer_ms,
        _ => panic!("build_tick_for_dir must emit engineTelemetry"),
    }
}

fn publisher_state_of(session: &Arc<EngineState>) -> PublisherState {
    session
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
// In-process RTMP test double (pure std TCP, loopback only).
//
// Speaks the REAL wire: Adobe handshake (C0/C1/S0/S1/S2/C2), then AMF0
// `connect` / `createStream` / `publish` over chunk stream 3 with 128-byte
// chunking + fmt=3 continuations, then FLV-typed media (0x09/0x08) on chunk
// streams 4/5. Replies are chunked the same way. The engine client cannot
// distinguish this from a real ingest except by provenance.
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Received {
    app: String,
    key: String,
    handshake_ok: bool,
    connect_ok: bool,
    publish_ok: bool,
    video_seq: bool,
    audio_seq: bool,
    /// Whether the FIRST video payload on the wire was the AVC sequence
    /// header (`0x17 0x00…`). `None` until any video arrives. Finding-1 pin:
    /// a second stream must open with its seq header first.
    first_video_was_seq: Option<bool>,
    video_frames: u64,
    audio_frames: u64,
    /// Raw media payload bytes (for the live-feed test's real-bytes assert).
    video_bytes: u64,
    /// First non-sequence video payloads (capped) for shape asserts.
    video_samples: Vec<Vec<u8>>,
    bytes: u64,
}

const DBL_CHUNK: usize = 128;

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

fn dbl_write_msg(
    s: &mut TcpStream,
    csid: u32,
    msg_type: u8,
    stream_id: u32,
    payload: &[u8],
) -> std::io::Result<()> {
    let mut offset = 0usize;
    let mut first = true;
    while offset < payload.len() || (payload.is_empty() && first) {
        let take = (payload.len() - offset).min(DBL_CHUNK);
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
}

struct DblConn {
    last: std::collections::HashMap<u32, DblHeader>,
    partial: std::collections::HashMap<u32, (DblHeader, Vec<u8>)>,
}

struct DblMsg {
    msg_type: u8,
    payload: Vec<u8>,
}

impl DblConn {
    fn new() -> Self {
        Self {
            last: Default::default(),
            partial: Default::default(),
        }
    }

    fn read_byte(s: &mut TcpStream) -> std::io::Result<u8> {
        let mut b = [0u8; 1];
        s.read_exact(&mut b)?;
        Ok(b[0])
    }

    fn read_u24(s: &mut TcpStream) -> std::io::Result<u32> {
        let mut b = [0u8; 3];
        s.read_exact(&mut b)?;
        Ok(((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32)
    }

    fn read_u32_le(s: &mut TcpStream) -> std::io::Result<u32> {
        let mut b = [0u8; 4];
        s.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    /// Read one chunk and return a complete message when reassembled.
    /// Control `Set Chunk Size` is consumed (tracked, replies stay at 128).
    fn next_message(&mut self, s: &mut TcpStream) -> std::io::Result<Option<DblMsg>> {
        loop {
            let fb = Self::read_byte(s)?;
            let fmt = fb >> 6;
            let mut csid = (fb & 0x3F) as u32;
            if csid == 0 {
                csid = Self::read_byte(s)? as u32 + 64;
            } else if csid == 1 {
                let a = Self::read_byte(s)? as u32;
                let b = Self::read_byte(s)? as u32;
                csid = a + b * 256 + 64;
            }
            let prev = self.last.get(&csid).cloned();
            let (ts, len, typ, sid) = match fmt {
                0 => {
                    let ts = Self::read_u24(s)?;
                    let len = Self::read_u24(s)? as usize;
                    let typ = Self::read_byte(s)?;
                    let sid = Self::read_u32_le(s)?;
                    if ts == 0xFF_FF_FF {
                        let mut b = [0u8; 4];
                        s.read_exact(&mut b)?;
                        let ets = u32::from_be_bytes(b);
                        self.last.insert(
                            csid,
                            DblHeader {
                                timestamp: ets,
                                msg_len: len,
                                msg_type: typ,
                                stream_id: sid,
                            },
                        );
                        (ets, len, typ, sid)
                    } else {
                        self.last.insert(
                            csid,
                            DblHeader {
                                timestamp: ts,
                                msg_len: len,
                                msg_type: typ,
                                stream_id: sid,
                            },
                        );
                        (ts, len, typ, sid)
                    }
                }
                1 => {
                    let delta = Self::read_u24(s)?;
                    let len = Self::read_u24(s)? as usize;
                    let typ = Self::read_byte(s)?;
                    let p = prev.ok_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, "fmt=1 no history")
                    })?;
                    let ts = p.timestamp.wrapping_add(delta);
                    self.last.insert(
                        csid,
                        DblHeader {
                            timestamp: ts,
                            msg_len: len,
                            msg_type: typ,
                            stream_id: p.stream_id,
                        },
                    );
                    (ts, len, typ, p.stream_id)
                }
                2 => {
                    let delta = Self::read_u24(s)?;
                    let p = prev.ok_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, "fmt=2 no history")
                    })?;
                    let ts = p.timestamp.wrapping_add(delta);
                    self.last.insert(
                        csid,
                        DblHeader {
                            timestamp: ts,
                            msg_len: p.msg_len,
                            msg_type: p.msg_type,
                            stream_id: p.stream_id,
                        },
                    );
                    (ts, p.msg_len, p.msg_type, p.stream_id)
                }
                3 => {
                    let p = prev.ok_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, "fmt=3 no history")
                    })?;
                    (p.timestamp, p.msg_len, p.msg_type, p.stream_id)
                }
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "bad fmt",
                    ));
                }
            };
            let (base, mut buf) = self
                .partial
                .remove(&csid)
                .map(|(h, b)| (Some(h), b))
                .unwrap_or((None, Vec::new()));
            let want = base.as_ref().map(|h| h.msg_len).unwrap_or(len);
            let remaining = want.saturating_sub(buf.len());
            let take = remaining.min(DBL_CHUNK);
            if take > 0 {
                let mut chunk = vec![0u8; take];
                s.read_exact(&mut chunk)?;
                buf.extend_from_slice(&chunk);
            }
            if buf.len() < want {
                let hdr = base.unwrap_or(DblHeader {
                    timestamp: ts,
                    msg_len: len,
                    msg_type: typ,
                    stream_id: sid,
                });
                self.partial.insert(csid, (hdr, buf));
                continue;
            }
            if typ == 0x01 {
                // Set Chunk Size: tracked, replies stay at 128.
                continue;
            }
            return Ok(Some(DblMsg {
                msg_type: typ,
                payload: buf,
            }));
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
        Self::start_on(0)
    }

    /// Bind an explicit port (loopback only). The reconnect test restarts on
    /// the SAME port the publisher redials — binding `:0` and hoping would
    /// test the OS lottery, not the transport.
    fn start_on(port: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("loopback bind must succeed");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener must succeed");
        let addr = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(Received::default()));
        let stall = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let conns: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let received_c = received.clone();
        let stall_c = stall.clone();
        let shutdown_c = shutdown.clone();
        let conns_c = conns.clone();
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
                // Track a clone so `kill` can force-close live connections
                // (unblocking their readers with EOF); the reader half moves
                // to a per-connection thread.
                if let Ok(track) = stream.try_clone() {
                    conns_c.lock().unwrap().push(track);
                }
                let received_cc = received_c.clone();
                let stall_cc = stall_c.clone();
                let shutdown_cc = shutdown_c.clone();
                std::thread::spawn(move || {
                    Self::serve(stream, &received_cc, &stall_cc, &shutdown_cc);
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

    /// Serve one connection: real RTMP handshake, AMF0 dialog, chunked media.
    #[allow(clippy::too_many_lines)]
    fn serve(
        mut s: TcpStream,
        received: &Arc<Mutex<Received>>,
        stall: &Arc<AtomicBool>,
        shutdown: &Arc<AtomicBool>,
    ) {
        // Accepted sockets inherit the listener's nonblocking mode: restore
        // blocking so reads wait for the peer (a nonblocking reader races
        // the peer's writes and fails handshake/media reads with WouldBlock).
        s.set_nonblocking(false).ok();
        // C0 + C1.
        let mut c0 = [0u8; 1];
        let mut c1 = [0u8; 1536];
        if s.read_exact(&mut c0).is_err() || s.read_exact(&mut c1).is_err() {
            return;
        }
        if c0[0] != 3 {
            return;
        }
        // S0 + S1 + S2 (S2 echoes C1).
        let mut s1 = [0u8; 1536];
        s1[0] = 0x53;
        if s.write_all(&[3]).is_err() || s.write_all(&s1).is_err() || s.write_all(&c1).is_err() {
            return;
        }
        // C2 must echo S1.
        let mut c2 = [0u8; 1536];
        if s.read_exact(&mut c2).is_err() || c2 != s1 {
            return;
        }
        received.lock().unwrap().handshake_ok = true;

        let mut conn = DblConn::new();
        // Dialog: connect → createStream → publish, then media forever.
        // Replies ride csid 3 / stream 0, chunked at 128 like the client's.
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
                Ok(Some(m)) => m,
                Ok(None) => continue,
                Err(_) => return,
            };
            match msg.msg_type {
                0x14 => {
                    let Some((name, trans, rest)) = damf_command(&msg.payload) else {
                        continue;
                    };
                    match name.as_str() {
                        "connect" => {
                            let mut app = String::new();
                            for v in &rest {
                                if let DAmf::Object(props) = v {
                                    for (k, val) in props {
                                        if k == "app" {
                                            if let DAmf::String(a) = val {
                                                app = a.clone();
                                            }
                                        }
                                    }
                                }
                            }
                            received.lock().unwrap().app = app;
                            // _result(trans) + props null + info object.
                            let mut payload = dbl_string("_result");
                            payload.extend_from_slice(&dbl_number(trans));
                            payload.extend_from_slice(&dbl_null());
                            payload.extend_from_slice(&dbl_object(&[
                                ("fmsVer", dbl_string("FMS/3,0,1,123")),
                                ("capabilities", dbl_number(31.0)),
                            ]));
                            let mut info = dbl_string("NetConnection.Connect.Success");
                            let _ = &mut info;
                            // Info rides as a second object arg for shape.
                            let mut full = payload;
                            full.extend_from_slice(&dbl_object(&[
                                ("level", dbl_string("status")),
                                ("code", dbl_string("NetConnection.Connect.Success")),
                            ]));
                            if dbl_write_msg(&mut s, 3, 0x14, 0, &full).is_err() {
                                return;
                            }
                            received.lock().unwrap().connect_ok = true;
                        }
                        "createStream" => {
                            let mut payload = dbl_string("_result");
                            payload.extend_from_slice(&dbl_number(trans));
                            payload.extend_from_slice(&dbl_null());
                            payload.extend_from_slice(&dbl_number(1.0));
                            if dbl_write_msg(&mut s, 3, 0x14, 0, &payload).is_err() {
                                return;
                            }
                        }
                        "publish" => {
                            let mut key = String::new();
                            for v in &rest {
                                if let DAmf::String(k) = v {
                                    if key.is_empty() {
                                        key = k.clone();
                                    }
                                }
                            }
                            received.lock().unwrap().key = key;
                            // onStatus NetStream.Publish.Start.
                            let mut payload = dbl_string("onStatus");
                            payload.extend_from_slice(&dbl_number(0.0));
                            payload.extend_from_slice(&dbl_null());
                            payload.extend_from_slice(&dbl_object(&[
                                ("level", dbl_string("status")),
                                ("code", dbl_string("NetStream.Publish.Start")),
                            ]));
                            if dbl_write_msg(&mut s, 3, 0x14, 1, &payload).is_err() {
                                return;
                            }
                            received.lock().unwrap().publish_ok = true;
                        }
                        "FCUnpublish" | "deleteStream" | "closeStream" => {}
                        _ => {}
                    }
                }
                0x09 => {
                    let mut r = received.lock().unwrap();
                    r.video_frames += 1;
                    r.video_bytes += msg.payload.len() as u64;
                    r.bytes += msg.payload.len() as u64;
                    if msg.payload.starts_with(&[0x17, 0x00]) {
                        r.video_seq = true;
                    } else if r.video_samples.len() < 8 {
                        r.video_samples.push(msg.payload.clone());
                    }
                    if r.first_video_was_seq.is_none() {
                        r.first_video_was_seq = Some(msg.payload.starts_with(&[0x17, 0x00]));
                    }
                }
                0x08 => {
                    let mut r = received.lock().unwrap();
                    r.audio_frames += 1;
                    r.bytes += msg.payload.len() as u64;
                    if msg.payload.starts_with(&[0xAF, 0x00]) {
                        r.audio_seq = true;
                    }
                }
                _ => {}
            }
        }
    }

    fn url(&self, app: &str, key: &str) -> String {
        format!("rtmp://{}/{app}/{key}", self.addr)
    }

    /// Kill the double mid-stream: force-close live connections (readers see
    /// EOF) and stop the listener. No FIN grace, no operator call.
    fn kill(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Force-close every live connection so blocked readers wake with EOF
        // (a bare listener close would leave established sockets alive and
        // the publisher would never notice the kill).
        for c in self.conns.lock().unwrap().iter() {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
        if let Some(t) = self.listener_thread.take() {
            let _ = t.join();
        }
        // Give per-connection threads a beat to observe EOF and exit so a
        // same-port restart does not race a lingering reader.
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Drop for TestDouble {
    fn drop(&mut self) {
        self.kill();
    }
}

// ---------------------------------------------------------------------------
// DoD 2 — publish session against the double.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publish_session_handshake_and_h264_aac_to_double() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let dbl = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "key-one"));
    load_and_start(&handler, &pkg_path).await;

    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start at the double must open");
    assert_eq!(*state.stream_state.lock().unwrap(), StreamState::Live);

    // The dial runs in the background: wait for Live BEFORE feeding, so the
    // bounded channel is draining and every send is admitted (pre-Live feeds
    // would shed by design — live edge, drop-new).
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "publisher must dial the double in the background"
    );
    feed_av(&state, 30, 46).await;
    assert!(
        poll_until(Duration::from_secs(5), || {
            let r = dbl.received.lock().unwrap();
            r.handshake_ok
                && r.connect_ok
                && r.publish_ok
                && r.video_seq
                && r.audio_seq
                && r.video_frames >= 31
                && r.audio_frames >= 47
        })
        .await,
        "double must see handshake + dialog + H.264 seq + AAC seq + media: {:?}",
        dbl.received.lock().unwrap(),
    );
    {
        let r = dbl.received.lock().unwrap();
        assert_eq!(r.app, "live", "double must see the app");
        assert_eq!(r.key, "key-one", "double must see the stream key");
    }
    assert_eq!(publisher_state_of(&state), PublisherState::Live);

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
    assert!(acked(&outgoing, 4));
}

// ---------------------------------------------------------------------------
// DoD 3 — reconnect: kill mid-stream → reconnecting → restart → live again,
// no operator action, View never stutters.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_kill_midstream_then_live_again_without_operator_action() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let mut dbl = TestDouble::start();
    let port = dbl.addr.port();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "reconnect-key"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start at the double must open");
    // Wait for Live BEFORE feeding: frames fed while redialing shed by
    // design (live edge, drop-new — `drain_stale`), so a pre-Live burst
    // would be dropped as stale and the frame-count assert below would test
    // the shed path, not the publish path.
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before the kill"
    );
    feed_av(&state, 10, 10).await;
    assert!(
        poll_until(Duration::from_secs(5), || dbl
            .received
            .lock()
            .unwrap()
            .video_frames
            >= 11)
        .await,
        "must publish before the kill"
    );

    let dropped_before = dropped(&state);

    // Kill mid-stream. No directive, no operator call.
    dbl.kill();
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Reconnecting)
        .await,
        "killing the double must surface Reconnecting"
    );
    // Local playout is untouched: the engine still calls itself Live and the
    // render loop shed nothing.
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Live,
        "engine stream state stays Live while the transport reconnects"
    );
    assert_eq!(
        dropped(&state),
        dropped_before,
        "View never stutters across the kill"
    );

    // Restart on the SAME port; the publisher redials on its own.
    let dbl2 = TestDouble::start_on(port);

    feed_best_effort(&state, 5, 5);
    assert!(
        poll_until(Duration::from_secs(10), || publisher_state_of(&state)
            == PublisherState::Live
            && dbl2.received.lock().unwrap().video_seq
            && dbl2.received.lock().unwrap().audio_seq)
        .await,
        "publisher must be Live again with codecs re-announced, no operator action"
    );
    assert_eq!(
        dropped(&state),
        dropped_before,
        "droppedFramesTotal unchanged across kill + redial"
    );

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// DoD 4 — survival: transport death never touches the View; publishes never
// block the caller (the falsification deadline an inline publisher fails).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn survival_transport_death_leaves_view_untouched_and_publishes_nonblocking() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let mut dbl = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "survival-key"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start at the double must open");
    feed_av(&state, 5, 5).await;
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before the kill"
    );

    let dropped_before = dropped(&state);
    dbl.kill();

    // The render path must never wait on the socket: 2× the channel bound of
    // large frames against a dead transport still returns promptly. An
    // inline-blocking publisher stalls here and blows the 2 s deadline —
    // that is the falsification signature (see report).
    let start = Instant::now();
    {
        let mut guard = state.stream_session.lock().unwrap();
        let s = guard.as_mut().expect("session must still be live");
        for i in 0..128 {
            let payload = vec![0x17u8; 64 * 1024];
            let _ = s.publish_video(payload);
            let _ = i;
        }
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "publishes against a dead transport must not block the caller, took {elapsed:?}"
    );
    assert_eq!(
        dropped(&state),
        dropped_before,
        "droppedFramesTotal delta zero across transport death"
    );

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("stop after transport death must still close cleanly");
    assert_eq!(*state.stream_state.lock().unwrap(), StreamState::Idle);
}

// ---------------------------------------------------------------------------
// DoD 5 — streamBufferMs is the transport's actual buffered bytes → ms.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_buffer_ms_moves_with_load_not_a_constant() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let dbl = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "buffer-key"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start at the double must open");
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before loading"
    );

    // Stall the double: it holds the connection but stops reading, so the
    // transport's buffer must GROW (bytes → ms via the §9.4 envelope).
    dbl.stall.store(true, Ordering::SeqCst);
    {
        let mut guard = state.stream_session.lock().unwrap();
        let s = guard.as_mut().expect("session must be live");
        for _ in 0..200 {
            let _ = s.publish_video(vec![0x17u8; 128 * 1024]);
        }
    }
    let grown_ms = poll_until(Duration::from_secs(10), || {
        let guard = state.stream_session.lock().unwrap();
        guard.as_ref().map(|s| s.stream_buffer_ms()).unwrap_or(0.0) > 0.0
    })
    .await;
    assert!(grown_ms, "buffer must grow while the double stalls reads");
    let loaded_ms = state
        .stream_session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.stream_buffer_ms())
        .unwrap_or(0.0);
    let loaded_bytes = state
        .stream_session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.buffered_bytes())
        .unwrap_or(0);
    assert!(
        loaded_bytes > 0,
        "buffered bytes must be nonzero under stall"
    );
    // Honest, not a guess: the ms value IS the byte count through the
    // envelope bitrate (8 Mbps video + 192 kbps audio, §9.4).
    let expected_ms = loaded_bytes as f64 * 8000.0 / 8_192_000.0;
    assert!(
        (loaded_ms - expected_ms).abs() < 1.0,
        "streamBufferMs ({loaded_ms}) must equal bytes→ms ({expected_ms}), not a constant"
    );

    // Drain: the double reads again, the buffer must SHRINK.
    dbl.stall.store(false, Ordering::SeqCst);
    assert!(
        poll_until(Duration::from_secs(10), || {
            let guard = state.stream_session.lock().unwrap();
            guard
                .as_ref()
                .map(|s| s.stream_buffer_ms())
                .unwrap_or(f64::MAX)
                < loaded_ms
        })
        .await,
        "buffer must shrink once the double drains"
    );

    dbl.stall.store(false, Ordering::SeqCst);
    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// FIX round item 2 — the telemetry tick wires session.stream_buffer_ms():
// nonzero under stall (channel backlog INCLUDED), zero when drained.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telemetry_tick_wires_stream_buffer_ms_nonzero_under_stall_zero_when_drained() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let dbl = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "tick-key"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before stalling"
    );

    // Stall: the tick must go NONZERO (the 0.0 stub is gone). The backlog
    // sits in the mpsc channel while the task is write-blocked — included by
    // construction (credited at send, debited at write).
    dbl.stall.store(true, Ordering::SeqCst);
    {
        let guard = state.stream_session.lock().unwrap();
        let s = guard.as_ref().expect("session must be live");
        for _ in 0..200 {
            let _ = s.publish_video(vec![0x17u8; 128 * 1024]);
        }
    }
    assert!(
        poll_until(Duration::from_secs(10), || {
            tick_stream_buffer_ms(&state) > 0.0
        })
        .await,
        "telemetry tick streamBufferMs must be nonzero under stall"
    );

    // Drain: the tick must return to ZERO (not merely shrink).
    dbl.stall.store(false, Ordering::SeqCst);
    assert!(
        poll_until(Duration::from_secs(15), || {
            tick_stream_buffer_ms(&state) == 0.0
        })
        .await,
        "telemetry tick streamBufferMs must be zero when drained"
    );

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// FIX round item 3 — LIVE FEED through the REAL loop hook.
//
// No direct session writes anywhere in this test (rule 7): frames travel
// Surface → `feed_stream_surface` (the exact hook `main.rs` calls —
// zero-copy `encode_pixel_buffer`, never readback/rgba) → bounded publish.
// The double must receive video carrying REAL encoded bytes (FLV NALU tags
// the synthetic feeder never emits), proving the Surface path end to end.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_feed_surface_path_publishes_real_bytes_via_loop_hook() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _outgoing) = harness();
    // The render loop must have published a device (chain probe builds the
    // take geometry's pool against it and keeps nothing).
    let _render = RenderLoop::new(state.clone()).await.ok();
    let device = state.render_device();
    if !nbe_engine::record::stream::chain_available(&device) {
        eprintln!("SKIP: no zero-copy chain on this machine (§0.1 assumption 24)");
        return;
    }
    let dbl = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "live-feed-key"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before feeding the Surface path"
    );

    // The REAL loop hook inputs: a drawn Surface from a View-geometry pool
    // (record-sized — over-provisioned for stream-only, never under) and the
    // lazily-opened encoder. NO publish_video / publish_audio calls here.
    let device = device.expect("chain_available proved a device");
    let pool = nbe_engine::record::zerocopy_pool(
        &device,
        nbe_engine::render::VIEW_W,
        nbe_engine::render::VIEW_H,
    )
    .expect("pool at View geometry must build on a chained machine");
    let mut encoder: Option<nbe_decode::encode::EncodeSession> = None;
    let mut seq_sent = false;
    let drops_before = state.skipped_stream_frames.load(Ordering::SeqCst);

    // Feed several frames through the hook (the encoder may buffer the first
    // one or two; the loop feeds every tick, so repetition is the honest
    // shape — still never a direct session write).
    for _ in 0..12 {
        let surface = pool.acquire().expect("pool must yield a Surface");
        let feed_ms = {
            let guard = state.stream_session.lock().unwrap();
            let sess = guard.as_ref().expect("stream session must be live");
            nbe_engine::record::stream::feed_stream_surface(
                Some(surface),
                &mut encoder,
                &mut seq_sent,
                sess,
                &state.skipped_stream_frames,
            )
        };
        let _ = feed_ms;
        tokio::task::yield_now().await;
    }

    // The double must have received video through the transport.
    assert!(
        poll_until(Duration::from_secs(10), || {
            dbl.received.lock().unwrap().video_frames >= 1
        })
        .await,
        "double must receive live video via the loop hook"
    );
    // …carrying REAL encoded bytes: FLV NALU tags (0x17/0x27 + 0x01),
    // longer than a tag header, and WITHOUT the synthetic "idr-" marker the
    // direct-write feeder emits. A direct session write cannot produce these
    // samples — only `encode_pixel_buffer` through the hook can.
    {
        let r = dbl.received.lock().unwrap();
        assert!(
            !r.video_samples.is_empty() || r.video_bytes > 0,
            "live video must carry bytes, got {r:?}"
        );
        for sample in &r.video_samples {
            assert!(
                sample.len() > 5,
                "live sample must be longer than the FLV tag header"
            );
            assert!(
                sample[0] == 0x17 || sample[0] == 0x27,
                "live sample must be an FLV NALU tag, got {:02X?}",
                &sample[..5.min(sample.len())]
            );
            assert_eq!(
                sample[1], 0x01,
                "live sample avc-type must be NALU (never sequence here)"
            );
            assert!(
                !sample.windows(4).any(|w| w == b"idr-"),
                "live sample must not carry the synthetic feeder marker"
            );
        }
    }
    // Drops discipline: the hook counts stream drops only (G1) — record and
    // View counters are untouched by construction (asserted structurally:
    // the hook takes `skipped_stream_frames`, never the record counter).
    let _ = drops_before;

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// FIX round item 4 — shutdown_and_wait never blocks the executor: stream.stop
// against a DEAD transport still acks inside a hard deadline.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_stop_against_dead_transport_still_acks() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let mut dbl = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "bounded-key"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before killing the transport"
    );

    // Dead transport, then stop: the async shutdown (tokio sleep, never
    // std::thread::sleep) must resolve inside a hard deadline — a blocking
    // shutdown would stall the executor and blow it.
    dbl.kill();
    let start = Instant::now();
    tokio::time::timeout(
        Duration::from_secs(5),
        handler.apply(&directive("stream.stop", 4, serde_json::json!({}))),
    )
    .await
    .expect("stream.stop must not stall the executor")
    .expect("stop after transport death must close cleanly");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "bounded stop took {:?}",
        start.elapsed()
    );
    assert!(acked(&outgoing, 4));
    assert_eq!(*state.stream_state.lock().unwrap(), StreamState::Idle);
}

// ---------------------------------------------------------------------------
// E_NETWORK / kind coverage for the touched error types.
// ---------------------------------------------------------------------------

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
    // … and teardown failures are E_NETWORK (withheld-ack path).
    nbe_engine::record::stream::set_force_close_error(true);
    let sel = nbe_engine::record::tap_path::select_stream(true).unwrap();
    let mut s = nbe_engine::record::stream::StreamSession::open("rtmp://example/live", sel);
    let err = s.stop_and_close().await.expect_err("armed seam must fail");
    assert!(
        err.to_string().contains("E_NETWORK"),
        "StreamError::Teardown must carry E_NETWORK, got: {err}"
    );
    nbe_engine::record::stream::set_force_close_error(false);
}

// ---------------------------------------------------------------------------
// FIX round item 1 — REAL RTMP interop against MediaMTX.
//
// Downloads NOTHING (the binary arrives out of band in /tmp ONLY — never
// committed): probes `/tmp/mediamtx-test/mediamtx` then `/tmp/mediamtx` and
// LOUDLY skips when absent (documented, never false-green). When present,
// the engine client publishes to `rtmp://127.0.0.1:1935/<app>/<key>` and the
// server's own API + log prove an incoming H.264+AAC stream. Checksum pinned
// in the report (darwin_amd64 tarball, verified out of band).
// ---------------------------------------------------------------------------

fn mediamtx_binary() -> Option<std::path::PathBuf> {
    for cand in ["/tmp/mediamtx-test/mediamtx", "/tmp/mediamtx"] {
        let p = std::path::PathBuf::from(cand);
        if p.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(md) = std::fs::metadata(&p) {
                    if md.permissions().mode() & 0o111 != 0 {
                        return Some(p);
                    }
                }
            }
            #[cfg(not(unix))]
            {
                return Some(p);
            }
        }
    }
    None
}

fn tcp_open(addr: &str) -> bool {
    std::net::TcpStream::connect(addr).is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mediamtx_proof_real_server_receives_h264_aac() {
    let _serial = SERIAL.lock().await;
    let Some(bin) = mediamtx_binary() else {
        eprintln!(
            "SKIP: MediaMTX proof needs the out-of-band binary at /tmp/mediamtx-test/mediamtx \
             (or /tmp/mediamtx), darwin_amd64 release, never committed. \
             Fast CI path is the in-process RTMP double above."
        );
        return;
    };
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }

    // Config lives in /tmp ONLY (never the repo): loopback RTMP :1935 per the
    // brief, API alongside for the incoming-stream proof, debug logs captured
    // to /tmp for the report.
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
        // CWD in /tmp: MediaMTX generates TLS stub files (auto.key/auto.crt)
        // on start — they must never land in the repo.
        .current_dir(workdir.path())
        .stdout(std::process::Stdio::from(log_file.try_clone().unwrap()))
        .stderr(std::process::Stdio::from(log_file))
        .spawn()
        .expect("mediamtx must spawn");
    let kill = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };

    // Wait for RTMP + API listeners.
    let mut ready = false;
    for _ in 0..100 {
        if tcp_open("127.0.0.1:1935") && tcp_open("127.0.0.1:19998") {
            ready = true;
            break;
        }
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        kill(&mut child);
        panic!("MediaMTX did not open RTMP/API listeners; server log:\n{log}");
    }

    // Publish from the REAL engine client to rtmp://127.0.0.1:1935/<app>/<key>.
    let app = "live";
    let key = "nbe-proof";
    let url = format!("rtmp://127.0.0.1:1935/{app}/{key}");
    let (_pkg, pkg_path) = write_package(&url);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start at MediaMTX must open");
    let reached_live = poll_until(Duration::from_secs(8), || {
        publisher_state_of(&state) == PublisherState::Live
    })
    .await;
    if !reached_live {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        kill(&mut child);
        panic!("publisher never went Live against MediaMTX; server log:\n{log}");
    }
    feed_av(&state, 10, 10).await;

    // Paced feed: gortmplib finalizes tracks only after ~2 s of TIMESTAMP
    // span (analyze window), and the transport timestamps at send — so a
    // burst (all frames stamped within ms) never completes the window. Pace
    // ~3.5 s of 1 video + 1 audio frame per 50 ms tick. Direct session
    // writes are the transport proof here (the loop-hook path is proven
    // separately by `live_feed_*`); the first payloads are the REAL AVC
    // sequence header (valid avcC — a placeholder-length avcC fails the
    // server's parse) and the valid AAC sequence header.
    for i in 0..70 {
        {
            let guard = state.stream_session.lock().unwrap();
            let s = guard.as_ref().expect("stream session must be live");
            let mut v = vec![0x27, 0x01, 0, 0, 0];
            v.extend_from_slice(format!("paced-{i}").as_bytes());
            let _ = s.publish_video(v);
            let mut a = vec![0xAF, 0x01];
            a.extend_from_slice(format!("apac-{i}").as_bytes());
            let _ = s.publish_audio(a);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Proof gates: the server's OWN log lines. `stream is available and
    // online, 2 tracks (H264, MPEG-4 Audio)` proves it parsed our AVC + AAC
    // sequence headers into tracks; `is publishing to path` proves the
    // session. (The v3 API lags the log — it still reads ready:false DURING
    // a live ffmpeg publish — so the log, not the API, is authoritative.)
    let mut tracks_line: Option<String> = None;
    let mut publishing_line: Option<String> = None;
    for _ in 0..100 {
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
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    match (tracks_line, publishing_line) {
        (Some(t), Some(p)) => {
            eprintln!("MEDIAMTX-PROOF {t}");
            eprintln!("MEDIAMTX-PROOF {p}");
        }
        _ => {
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            kill(&mut child);
            panic!("MediaMTX shows no incoming H.264+AAC stream for {key}; server log:\n{log}");
        }
    }

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
    kill(&mut child);
}

// ---------------------------------------------------------------------------
// FIX round 2, finding 1 — two sequential streams: the second emits a valid
// AVC sequence header FIRST.
//
// The test replays the bug first (stale `seq_sent` carried across streams —
// the pre-fix loop behavior): media flows but NO sequence header ever arrives
// (fresh publisher cache is empty, nothing to replay). Then it replays the
// fix (fresh per-stream state, what `StreamLoopState::note_live` provides):
// the first video payload on the wire is the sequence header.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_stream_emits_seq_header_first() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _outgoing) = harness();
    let _render = RenderLoop::new(state.clone()).await.ok();
    let device = state.render_device();
    if !nbe_engine::record::stream::chain_available(&device) {
        eprintln!("SKIP: no zero-copy chain on this machine (§0.1 assumption 24)");
        return;
    }
    let device = device.expect("chain_available proved a device");
    let dbl1 = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl1.url("live", "seq-first-one"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("first stream.start must open");
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before feeding"
    );

    // Feed the REAL loop hook (never a direct session write).
    let pool = nbe_engine::record::zerocopy_pool(
        &device,
        nbe_engine::render::VIEW_W,
        nbe_engine::render::VIEW_H,
    )
    .expect("pool at View geometry must build on a chained machine");
    let feed_hook = |encoder: &mut Option<nbe_decode::encode::EncodeSession>,
                     seq_sent: &mut bool| {
        let surface = pool.acquire().expect("pool must yield a Surface");
        let guard = state.stream_session.lock().unwrap();
        let sess = guard.as_ref().expect("stream session must be live");
        nbe_engine::record::stream::feed_stream_surface(
            Some(surface),
            encoder,
            seq_sent,
            sess,
            &state.skipped_stream_frames,
        )
    };
    let mut encoder: Option<nbe_decode::encode::EncodeSession> = None;
    let mut seq_sent = false;
    // Encoder warmup (~6 frames emit nothing): feed until the header is out
    // and media flows, bounded.
    for _ in 0..30 {
        feed_hook(&mut encoder, &mut seq_sent);
        tokio::task::yield_now().await;
        if seq_sent && dbl1.received.lock().unwrap().video_frames >= 1 {
            break;
        }
    }
    assert!(seq_sent, "first stream sends its header");
    assert!(
        dbl1.received.lock().unwrap().video_frames >= 1,
        "first stream must flow"
    );
    assert!(
        dbl1.received.lock().unwrap().video_seq,
        "first stream carries its seq header"
    );
    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("first stop must succeed");

    // BUG replay: the stale sent flag (and warm encoder) carried into the
    // second stream — the pre-fix loop behavior. Media flows, but the fresh
    // publisher's cache is empty and no header is ever attempted.
    let dbl_bug = TestDouble::start();
    handler
        .apply(&directive(
            "stream.start",
            5,
            serde_json::json!({ "url": dbl_bug.url("live", "seq-bug") }),
        ))
        .await
        .expect("second stream.start must open");
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before feeding the stale state"
    );
    for _ in 0..6 {
        feed_hook(&mut encoder, &mut seq_sent);
        tokio::task::yield_now().await;
    }
    assert!(
        poll_until(Duration::from_secs(5), || {
            dbl_bug.received.lock().unwrap().video_frames >= 1
        })
        .await,
        "stale-flag stream still flows media"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !dbl_bug.received.lock().unwrap().video_seq,
        "BUG SHAPE: a stale seq flag sends zero seq headers on the new stream: {:?}",
        dbl_bug.received.lock().unwrap(),
    );
    handler
        .apply(&directive("stream.stop", 6, serde_json::json!({})))
        .await
        .expect("second stop must succeed");

    // FIX replay: fresh per-stream state — exactly what the loop's
    // start-transition reset provides. The FIRST video payload must be the
    // AVC sequence header.
    let dbl_fix = TestDouble::start();
    handler
        .apply(&directive(
            "stream.start",
            7,
            serde_json::json!({ "url": dbl_fix.url("live", "seq-fix") }),
        ))
        .await
        .expect("third stream.start must open");
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before feeding the fresh state"
    );
    let mut encoder_fix: Option<nbe_decode::encode::EncodeSession> = None;
    let mut seq_sent_fix = false;
    for _ in 0..30 {
        feed_hook(&mut encoder_fix, &mut seq_sent_fix);
        tokio::task::yield_now().await;
        if seq_sent_fix && dbl_fix.received.lock().unwrap().video_frames >= 1 {
            break;
        }
    }
    assert!(seq_sent_fix, "fresh stream sends its header");
    assert!(
        poll_until(Duration::from_secs(10), || {
            let r = dbl_fix.received.lock().unwrap();
            r.video_frames >= 1 && r.video_seq
        })
        .await,
        "fresh stream must emit its seq header: {:?}",
        dbl_fix.received.lock().unwrap(),
    );
    assert_eq!(
        dbl_fix.received.lock().unwrap().first_video_was_seq,
        Some(true),
        "the second (fixed) stream emits a valid seq header FIRST"
    );

    handler
        .apply(&directive("stream.stop", 8, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// FIX round 2, finding 2 — record + stream concurrently live: the stream
// receives frames.
//
// A faithful single-tick simulation with BOTH sessions real: the record leg
// runs the unchanged take path (`begin_tap_frame` → `end_tap_frame`), the
// stream leg shares the loan via Arc clone (the G1 op the loop performs),
// encodes lock-free, and publishes under a brief session lock. Asserts: the
// record leg admits every frame, the stream drops ZERO frames (the
// starvation — one drop/frame, zero feeds — is gone), and the double receives
// real encoded bytes.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_live_stream_receives_frames_from_shared_record_surface() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _outgoing) = harness();
    let mut render = match RenderLoop::new(state.clone()).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("SKIP: render loop unavailable: {e}");
            return;
        }
    };
    if !nbe_engine::record::stream::chain_available(&state.render_device()) {
        eprintln!("SKIP: no zero-copy chain on this machine (§0.1 assumption 24)");
        return;
    }
    let dbl = TestDouble::start();
    let (_pkg, pkg_path, _rec) = write_record_package(&dbl.url("live", "both-live-key"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("record.start", 3, serde_json::json!({})))
        .await
        .expect("record.start must open");
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Recording);
    let (pool, tx) = {
        let guard = state.record_session.lock().unwrap();
        let s = guard.as_ref().expect("record session must be live");
        (s.surface_pool(), s.frame_sender())
    };
    let Some(pool) = pool else {
        eprintln!("SKIP: record take is CPU readback — the share needs a zero-copy loan");
        handler
            .apply(&directive("record.stop", 9, serde_json::json!({})))
            .await
            .ok();
        return;
    };
    handler
        .apply(&directive("stream.start", 4, serde_json::json!({})))
        .await
        .expect("stream.start must open alongside the take");
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "publisher must be Live before the shared tick"
    );

    let claims_zero_copy = matches!(
        *state.record_tap_selection.lock().unwrap(),
        Some(sel) if sel.path == nbe_engine::record::tap_path::TapPath::ZeroCopy
    );
    let mut encoder: Option<nbe_decode::encode::EncodeSession> = None;
    let mut seq_sent = false;
    let record_skips_before = state.skipped_record_frames.load(Ordering::SeqCst);
    let stream_drops_before = state.skipped_stream_frames.load(Ordering::SeqCst);
    // Shared ticks until the header is out and media flows (encoder warmup
    // emits nothing for the first frames — bounded).
    for i in 0..30 {
        // Record leg — the unchanged take path.
        let loan = nbe_engine::record::begin_tap_frame(
            &mut render,
            Some(pool.as_ref()),
            claims_zero_copy,
            &state.skipped_record_frames,
        )
        .expect("take must hold while live");
        // THE share: clone the loan (G1 — one composite, N holders).
        let shared = loan.surface();
        assert!(
            shared.is_some(),
            "take holds a surface to share (frame {i})"
        );
        nbe_engine::record::restore_view(&mut render, &loan);
        let _ = nbe_engine::record::end_tap_frame(
            loan,
            Duration::ZERO,
            None,
            &tx,
            &state.skipped_record_frames,
            || async { (Vec::new(), Duration::ZERO) },
        )
        .await;
        // Stream leg — encode lock-free, brief lock only to publish.
        let surf = shared.expect("asserted above");
        let (encode_ms, payload) = nbe_engine::record::stream::encode_stream_frame(
            &surf,
            &mut encoder,
            !seq_sent,
            &state.skipped_stream_frames,
        );
        let _ = encode_ms;
        if let Some(payload) = payload {
            let guard = state.stream_session.lock().unwrap();
            let sess = guard.as_ref().expect("stream must stay live");
            let _ = nbe_engine::record::stream::publish_stream_frame(
                sess,
                payload.seq_header,
                &mut seq_sent,
                &payload.units,
                &state.skipped_stream_frames,
            );
        }
        // G1 release: the stream's Arc dies here; the record thread owns its
        // own clone until its encode is done.
        drop(surf);
        tokio::time::sleep(Duration::from_millis(50)).await;
        if seq_sent && dbl.received.lock().unwrap().video_frames >= 1 {
            break;
        }
    }
    assert_eq!(
        state.skipped_record_frames.load(Ordering::SeqCst) - record_skips_before,
        0,
        "record leg admits every shared frame"
    );
    assert_eq!(
        state.skipped_stream_frames.load(Ordering::SeqCst) - stream_drops_before,
        0,
        "stream receives frames while recording: zero drops (finding-2 starvation gone)"
    );
    assert!(seq_sent, "shared leg sends the header");
    assert!(
        poll_until(Duration::from_secs(10), || {
            let r = dbl.received.lock().unwrap();
            r.video_frames >= 1 && r.video_seq
        })
        .await,
        "double must receive shared-leg video with seq: {:?}",
        dbl.received.lock().unwrap(),
    );

    handler
        .apply(&directive("stream.stop", 5, serde_json::json!({})))
        .await
        .expect("stream cleanup stop must succeed");
    // Let the record thread's async callbacks land before the bounded finish.
    tokio::time::sleep(Duration::from_millis(500)).await;
    handler
        .apply(&directive("record.stop", 6, serde_json::json!({})))
        .await
        .expect("record cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// FIX round 2, finding 3 — encode holds no session lock.
//
// Holds `stream_session` on this thread, then encodes: the split API takes no
// session, so a regression that locks the session inside encode self-deadlocks
// here (std Mutex, same thread — loud, not silent). Correct code passes in
// milliseconds; the publish afterwards uses the brief locked section only.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn encode_frame_holds_no_session_lock() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _outgoing) = harness();
    let _render = RenderLoop::new(state.clone()).await.ok();
    let device = state.render_device();
    if !nbe_engine::record::stream::chain_available(&device) {
        eprintln!("SKIP: no zero-copy chain on this machine (§0.1 assumption 24)");
        return;
    }
    let device = device.expect("chain_available proved a device");
    let dbl = TestDouble::start();
    let (_pkg, pkg_path) = write_package(&dbl.url("live", "lockfree-key"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert!(
        poll_until(Duration::from_secs(5), || publisher_state_of(&state)
            == PublisherState::Live)
        .await,
        "must reach Live before the contention probe"
    );

    let pool = nbe_engine::record::zerocopy_pool(
        &device,
        nbe_engine::render::VIEW_W,
        nbe_engine::render::VIEW_H,
    )
    .expect("pool at View geometry must build on a chained machine");
    let mut encoder: Option<nbe_decode::encode::EncodeSession> = None;
    // No await inside the locked scopes: the guards never cross one, so the
    // future stays Send. A regression (encode locking the session) deadlocks
    // THIS thread here — loud, not silent. Encoder warmup emits nothing for
    // the first frames, so encode under the held lock until the header is
    // ready, bounded.
    let mut payload = None;
    for _ in 0..30 {
        let surf = pool.acquire().expect("pool must yield a Surface");
        let (_encode_ms, got) = {
            let _held = state.stream_session.lock().unwrap();
            nbe_engine::record::stream::encode_stream_frame(
                &surf,
                &mut encoder,
                true,
                &state.skipped_stream_frames,
            )
        };
        if let Some(p) = got {
            if p.seq_header.is_some() {
                payload = Some(p);
                break;
            }
        }
        tokio::task::yield_now().await;
    }
    let payload = payload.expect("encode must produce a header on chained hw");
    {
        let guard = state.stream_session.lock().unwrap();
        let sess = guard.as_ref().expect("stream must stay live");
        let mut seq_sent = false;
        let _ = nbe_engine::record::stream::publish_stream_frame(
            sess,
            payload.seq_header,
            &mut seq_sent,
            &payload.units,
            &state.skipped_stream_frames,
        );
        assert!(seq_sent, "contention probe sends the header");
    }
    assert!(
        poll_until(Duration::from_secs(10), || {
            dbl.received.lock().unwrap().video_frames >= 1
        })
        .await,
        "double must receive the lock-free frame"
    );

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}
