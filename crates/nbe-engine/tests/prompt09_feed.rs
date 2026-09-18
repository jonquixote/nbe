//! Prompt 09 WU-pipe (SPEC §9.3): main-loop record handoff + end-to-end.
//!
//! TDD: rewritten BEFORE the pipeline lands (RED first). The old inline
//! `feed_record_frame` (loop encodes, session buffers) is gone: the loop
//! pre-checks budget BEFORE the readback, reads back, and hands the frame to
//! the record thread over a bounded channel (shed + count on full). The thread
//! owns the ONE live encoder, muxes fragments immediately, and drains the tap.
//!
//! Coverage maps to the work-unit DoD:
//! 1. Live end-to-end through the REAL loop feed fns
//!    (`should_skip_record_frame` + `handoff_record_frame` + `stop_and_finish`):
//!    start → N frames → stop yields an ffprobe-parseable h264+aac file with
//!    tone and markers in the sidecar; `record_tap_ms` accumulates, render
//!    accounting unchanged.
//! 2. SIGKILL shape at pipeline level: dropping the session mid-take (no
//!    finish) keeps prior flushed fragments parseable.
//! 3. Skip policy: over-budget → skip before the readback (counter increments,
//!    View drops unchanged); full channel → shed + count, View untouched.
//! 4. Loop wiring ([`main.rs`]): the budget pre-check sits BEFORE the readback
//!    await, and the handoff is the only record cost folded into
//!    `record_tap_ms` (source assertion, same discipline as `prompt06.rs`).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::record::thread::RecordMsg;
use nbe_engine::record::AudioTap;
use nbe_engine::record::{handoff_record_frame, should_skip_record_frame, RECORD_CHANNEL_BOUND};
use nbe_engine::render::VIEW_H;
use nbe_engine::render::VIEW_W;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};

fn directive(
    command: &str,
    sv: u64,
    target: serde_json::Value,
    payload: serde_json::Value,
) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target,
        payload,
    }
}

fn harness() -> (Arc<EngineState>, DirectiveHandler, Arc<OutgoingQueue>) {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    (state, handler, outgoing)
}

fn hw_or_skip() -> bool {
    if nbe_engine::encode::is_available() {
        true
    } else {
        eprintln!("SKIP: no hardware H.264 encoder on this machine (SPEC §9.2); recording has no CPU fallback");
        false
    }
}

fn aac_or_skip() -> bool {
    if nbe_engine::record::aac::is_available() {
        true
    } else {
        eprintln!("SKIP: AudioToolbox AAC absent — feed tests need the encoder");
        false
    }
}

fn ffprobe_or_skip() -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from("/usr/local/bin/ffprobe");
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("SKIP: /usr/local/bin/ffprobe absent");
        None
    }
}

fn ffprobe_streams(ffprobe: &Path, file: &Path) -> serde_json::Value {
    let out = std::process::Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-show_streams")
        .arg("-show_format")
        .arg("-of")
        .arg("json")
        .arg(file)
        .output()
        .expect("spawning ffprobe must succeed");
    assert!(
        out.status.success(),
        "ffprobe must parse the recording, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("ffprobe JSON must parse")
}

/// Full-View synthetic RGBA: the handoff carries what the loop read back.
fn synthetic_rgba(frame: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((VIEW_W * VIEW_H * 4) as usize);
    for y in 0..VIEW_H {
        for x in 0..VIEW_W {
            out.push(((x + frame * 7) % 256) as u8);
            out.push(((y + frame * 13) % 256) as u8);
            out.push(((x + y + frame * 3) % 256) as u8);
            out.push(255);
        }
    }
    out
}

/// 440 Hz stereo tone, amplitude 0.5, interleaved f32.
fn tone_secs(secs: u32) -> Vec<f32> {
    let frames = (48_000 * secs) as usize;
    let mut pcm = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48_000.0).sin();
        pcm.push(v);
        pcm.push(v);
    }
    pcm
}

/// Top-level fourccs of a file, in order.
fn top_level_boxes(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let kind = String::from_utf8_lossy(&bytes[off + 4..off + 8]).into_owned();
        let len = if len == 1 {
            u64::from_be_bytes(bytes[off + 8..off + 16].try_into().unwrap()) as usize
        } else {
            len
        };
        assert!(len >= 8, "box {kind} has impossible length {len}");
        out.push(kind);
        off += len;
    }
    out
}

/// The record tests share process-wide seams (the marker store), so they
/// serialize on this lock (cf. `prompt09_markers.rs`).
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Start a recording through the directive path (SPEC §16.14 payload:
/// `{ outputId? }` only — naming/geometry/rate come from the package + engine).
async fn start_recording(
    state: &Arc<EngineState>,
    handler: &DirectiveHandler,
    sv: u64,
) -> std::path::PathBuf {
    handler
        .apply(&directive(
            "show.start",
            sv,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    // Leak: the session's output path lives under this dir past the stop.
    let dir = Box::leak(Box::new(dir));
    *state.record_dir.lock().unwrap() = Some(dir.path().to_path_buf());
    handler
        .apply(&directive(
            "record.start",
            sv + 1,
            serde_json::json!({}),
            serde_json::json!({"outputId": "ep01"}),
        ))
        .await
        .expect("record.start on a RUNNING show must open the pipeline");
    state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .expect("session must be present")
        .output_path()
        .to_path_buf()
}

/// Feed `n` frames through the REAL loop feed fns: budget pre-check (never
/// over here) + `handoff_record_frame` into the session's channel, folding
/// readback + handoff into the engine-state `record_tap_ms` exactly as the
/// loop does. Returns accumulated feed ms for the test's own assert.
fn feed_frames(state: &Arc<EngineState>, n: u32) -> f64 {
    let budget = Duration::from_secs_f64(1.0 / 30.0);
    let mut total = 0.0;
    for frame in 0..n {
        // Loop-measured render time: comfortably inside budget.
        let render_elapsed = Duration::from_millis(2);
        assert!(
            !should_skip_record_frame(render_elapsed, Some(budget)),
            "inside budget the pre-check must not skip"
        );
        // The readback (timed; its cost belongs to the record counter).
        let readback_started = Instant::now();
        let rgba = synthetic_rgba(frame);
        let readback_elapsed = readback_started.elapsed();
        let tx = state
            .record_session
            .lock()
            .unwrap()
            .as_ref()
            .expect("session must be present")
            .frame_sender();
        let outcome = handoff_record_frame(rgba, &tx, readback_elapsed);
        assert!(outcome.sent, "a live thread must accept handoffs");
        assert!(
            outcome.feed_ms >= readback_elapsed.as_secs_f64() * 1000.0,
            "feed_ms must include the measured readback"
        );
        *state.record_tap_ms.lock().unwrap() += outcome.feed_ms;
        total += outcome.feed_ms;
    }
    total
}

#[tokio::test]
async fn live_recording_end_to_end_through_real_feed_fns() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    let Some(ffprobe) = ffprobe_or_skip() else {
        return;
    };
    nbe_engine::record::markers::clear();
    let (state, handler, _) = harness();
    let video_path = start_recording(&state, &handler, 1).await;
    // Tone through the SHARED tap (the record-start wiring attached it; the
    // audio driver would feed it live — here the test pushes real PCM).
    let tap: Arc<AudioTap> = state.record_session.lock().unwrap().as_ref().unwrap().tap();
    tap.push(&tone_secs(2));
    handler
        .apply(&directive(
            "marker.add",
            3,
            serde_json::json!({}),
            serde_json::json!({"name": "chapter-1"}),
        ))
        .await
        .unwrap();
    let dropped_before = state
        .dropped_frames_total
        .load(std::sync::atomic::Ordering::SeqCst);
    // >1 s of video so a fragment flushes mid-take (crash-safe shape), tone
    // covers the span.
    let total_ms = feed_frames(&state, 40);
    assert!(
        total_ms > 0.0,
        "record_tap_ms must accumulate, got {total_ms}"
    );
    assert_eq!(
        *state.record_tap_ms.lock().unwrap(),
        total_ms,
        "loop folds every handoff into the engine-state counter"
    );

    handler
        .apply(&directive(
            "record.stop",
            4,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect("record.stop must finish the pipeline");

    // Render accounting untouched by the whole take.
    assert_eq!(
        state
            .dropped_frames_total
            .load(std::sync::atomic::Ordering::SeqCst),
        dropped_before,
        "record handoffs must never move View deadline accounting"
    );
    let v = ffprobe_streams(&ffprobe, &video_path);
    let streams = v["streams"].as_array().expect("streams array");
    assert_eq!(streams.len(), 2, "exactly 1 video + 1 audio stream");
    assert!(streams.iter().any(|s| s["codec_name"] == "h264"));
    assert!(streams.iter().any(|s| s["codec_name"] == "aac"));
    let sidecar = nbe_engine::record::markers::sidecar_path(&video_path);
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).expect("sidecar must exist"))
            .expect("sidecar JSON must parse");
    let entries = parsed["markers"].as_array().expect("markers array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "chapter-1");
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn sigkill_shape_drop_mid_take_keeps_prior_fragments() {
    // Pipeline-level crash shape: the session (senders + rendezvous) is
    // dropped mid-take with no finish. The thread exits without finalizing;
    // prior flushed fragments must still parse.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    let Some(ffprobe) = ffprobe_or_skip() else {
        return;
    };
    nbe_engine::record::markers::clear();
    let (state, handler, _) = harness();
    let video_path = start_recording(&state, &handler, 1).await;
    let tap: Arc<AudioTap> = state.record_session.lock().unwrap().as_ref().unwrap().tap();
    tap.push(&tone_secs(2));
    feed_frames(&state, 40);

    // Wait for a flushed fragment (bounded: the thread is live).
    let mut moof = false;
    for _ in 0..200 {
        if let Ok(bytes) = std::fs::read(&video_path) {
            if top_level_boxes(&bytes).iter().any(|k| k == "moof") {
                moof = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(moof, "thread must flush ≥1 fragment mid-take");

    // No finish: drop the pipeline mid-take (the SIGKILL shape).
    state.record_session.lock().unwrap().take();
    *state.record_state.lock().unwrap() = nbe_engine::state::RecordState::Idle;
    *state.record_tap.lock().unwrap() = None;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let bytes = std::fs::read(&video_path).expect("partial file must exist");
    let top = top_level_boxes(&bytes);
    assert_eq!(top.first().map(String::as_str), Some("ftyp"));
    assert!(top.iter().any(|k| k == "moov"), "moov upfront: init safe");
    assert!(
        top.iter().any(|k| k == "moof"),
        "a dropped take retains ≥1 flushed fragment"
    );
    let v = ffprobe_streams(&ffprobe, &video_path);
    let streams = v["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2, "partial file keeps both streams");
    assert!(streams.iter().any(|s| s["codec_name"] == "h264"));
    assert!(streams.iter().any(|s| s["codec_name"] == "aac"));
    nbe_engine::record::markers::clear();
}

#[test]
fn over_budget_pre_check_skips_before_any_readback_cost() {
    // Pure policy: the View already spent the budget → skip. The loop checks
    // this BEFORE the readback await, so a skipped record frame costs nothing
    // by definition (R4: record degrades, View never waits).
    assert!(should_skip_record_frame(
        Duration::from_secs(1),
        Some(Duration::from_nanos(1))
    ));
    assert!(should_skip_record_frame(
        Duration::from_secs_f64(1.0 / 30.0),
        Some(Duration::from_secs_f64(1.0 / 30.0))
    ));
    assert!(!should_skip_record_frame(
        Duration::from_millis(2),
        Some(Duration::from_secs_f64(1.0 / 30.0))
    ));
    // STOPPED: no deadline is real, always feed.
    assert!(!should_skip_record_frame(Duration::from_secs(1), None));
}

#[test]
fn full_channel_sheds_and_reports_for_the_skip_counter() {
    // Shed policy: a saturated record thread never blocks the loop — the
    // handoff reports `sent=false` and the loop counts the skip. No receiver
    // drains here, so the bound fills deterministically.
    let (tx, _rx) = std::sync::mpsc::sync_channel::<RecordMsg>(RECORD_CHANNEL_BOUND);
    for _ in 0..RECORD_CHANNEL_BOUND {
        tx.try_send(RecordMsg::Unit(fake_unit_for_shed(0.0)))
            .expect("bound must fill");
    }
    let outcome = handoff_record_frame(vec![0u8; 4], &tx, Duration::ZERO);
    assert!(!outcome.sent, "a full channel must shed, never block");
    assert_eq!(
        outcome.feed_ms, 0.0,
        "a shed handoff performed no readback (caller passes ZERO) and no send"
    );
}

fn fake_unit_for_shed(pts: f64) -> nbe_engine::encode::EncodedUnit {
    nbe_engine::encode::EncodedUnit {
        data: vec![0x00, 0x00, 0x00, 0x04, 0x41, 0x11, 0x22, 0x33],
        pts_seconds: pts,
        is_keyframe: false,
    }
}

#[test]
fn loop_wiring_pre_check_before_readback() {
    // Wiring proof (same discipline as `prompt06.rs` source assertions): the
    // budget pre-check sits BEFORE the readback await in the loop, and the
    // handoff is the only record cost folded into `record_tap_ms`.
    let src = include_str!("../src/main.rs");
    // Order inside the loop body (positional: each search starts where the
    // previous match ended, so the import lines cannot confuse it).
    let pre = src
        .find("should_skip_record_frame")
        .expect("loop must call the budget pre-check");
    let readback = src[pre..]
        .find("readback_view().await")
        .map(|i| pre + i)
        .expect("loop must read back for the record handoff");
    let handoff = src[readback..]
        .find("handoff_record_frame")
        .map(|i| readback + i)
        .expect("loop must hand off to the record thread");
    let counter = src[handoff..]
        .find("record_tap_ms")
        .map(|i| handoff + i)
        .expect("loop must accumulate the record counter");
    assert!(
        pre < readback,
        "pre-check (at {pre}) must precede the readback await (at {readback})"
    );
    assert!(
        readback < handoff,
        "handoff (at {handoff}) follows the readback (at {readback})"
    );
    assert!(
        handoff < counter,
        "counter fold (at {counter}) follows the handoff (at {handoff})"
    );
    assert!(
        !src.contains("feed_record_frame"),
        "the inline encode path must be gone from the loop"
    );
}
