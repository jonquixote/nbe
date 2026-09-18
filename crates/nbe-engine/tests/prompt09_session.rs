//! Prompt 09 WU-pipe (SPEC §16.14, §5.9.5): record pipeline glue —
//! `record.start` opens the dedicated record thread, `record.stop` finishes it
//! synchronously before the ack, `show.stop` quiesces per its payload.
//!
//! TDD: rewritten BEFORE the pipeline lands (RED first). The retired premise
//! below is the WU8 one-shot shape (session buffers + `finish` over buffers +
//! start refusing `E_NO_HARDWARE_ENCODER` only via a throwaway encode): the
//! pipeline instead refuses start on a failed encoder PROBE (SPEC §16.14
//! precondition, no stream opened), feeds frames over a bounded channel to the
//! thread, and finishes there.
//!
//! Coverage maps to the work-unit DoD:
//! 1. `record.start` on a RUNNING show with encoder available → `Recording`,
//!    session + shared tap present, output path reserved. Preconditions intact:
//!    not-running → `E_FORBIDDEN_STATE`; failed probe (seam-forced or genuine)
//!    → `E_NO_HARDWARE_ENCODER`.
//! 2. `record.stop` finishes synchronously (file + sidecar complete) and only
//!    then does the ack flow (`last_applied` + `AppliedStateVersion`). A failed
//!    finish surfaces (no ack) with the file kept as-is. Quiescence:
//!    `show.stop` graceful / immediate (`force`) / refused (`quiesceOutputs`
//!    false without force) per the §16.1 table.
//! 3. Second `record.start` while `Recording` → `E_FORBIDDEN_STATE`, pipeline
//!    preserved, chapters reset for the next take.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::directive::{DirectiveError, DirectiveHandler};
use nbe_engine::record::session as session_glue;
use nbe_engine::record::{handoff_record_frame, should_skip_record_frame};
use nbe_engine::state::{EngineState, OutgoingQueue, RecordState};
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION};

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

fn is_forbidden(err: &DirectiveError) -> bool {
    matches!(err, DirectiveError::ForbiddenState(_))
        && err.to_string().contains("E_FORBIDDEN_STATE")
}

/// SPEC §16.14 payload: `{ outputId? }` ONLY. Naming/geometry/rate come from
/// the loaded package + engine — legacy `show`/`episode`/`width`/`height`/
/// `fps`/`startTimestamp` fields are retired (see `on_record_start` docs).
fn start_payload() -> serde_json::Value {
    serde_json::json!({ "outputId": "ep01" })
}

fn acked(outgoing: &OutgoingQueue, sv: u64) -> bool {
    outgoing.drain().into_iter().any(|f| match f {
        EngineFrame::AppliedStateVersion { state_version, .. } => state_version == sv,
        _ => false,
    })
}

/// The record tests share process-wide seams (the force-no-encoder flag and
/// the marker store), so they serialize on this lock (cf. `prompt09_markers.rs`).
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Resets the force-no-encoder seam on drop so a panicking test cannot poison
/// later tests in this binary.
struct ForceNoEncoderGuard;
impl ForceNoEncoderGuard {
    fn set() -> Self {
        session_glue::set_force_no_encoder(true);
        Self
    }
}
impl Drop for ForceNoEncoderGuard {
    fn drop(&mut self) {
        session_glue::set_force_no_encoder(false);
    }
}

fn hw_or_skip() -> bool {
    if nbe_engine::encode::is_available() {
        return true;
    }
    eprintln!("SKIP: no hardware H.264 encoder on this machine (SPEC §9.2); recording has no CPU fallback");
    false
}

fn aac_or_skip() -> bool {
    if nbe_engine::record::aac::is_available() {
        true
    } else {
        eprintln!("SKIP: AudioToolbox AAC absent — session tests need the encoder");
        false
    }
}

fn synthetic_rgba(width: u32, height: u32, frame: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
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

/// Top-level fourccs of a file, in order (minimal box walk: no ffprobe needed
/// for the "file non-empty + moov present" leg of the DoD).
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

fn assert_moov_present(path: &Path) {
    let bytes = std::fs::read(path).expect("recording file must exist after finish");
    assert!(!bytes.is_empty(), "recording file must be non-empty");
    let top = top_level_boxes(&bytes);
    assert_eq!(top.first().map(String::as_str), Some("ftyp"));
    assert!(
        top.iter().any(|k| k == "moov"),
        "recording must carry moov, top-level: {top:?}"
    );
}

fn ffprobe_or_skip() -> Option<PathBuf> {
    let p = PathBuf::from("/usr/local/bin/ffprobe");
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("SKIP: /usr/local/bin/ffprobe absent — falling back to moov walk");
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

/// Start a recording through the directive path; returns the reserved output
/// path. The record dir is a leaked tempdir (the file outlives the stop).
async fn start_recording(state: &Arc<EngineState>, handler: &DirectiveHandler, sv: u64) -> PathBuf {
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
    let dir = Box::leak(Box::new(dir));
    *state.record_dir.lock().unwrap() = Some(dir.path().to_path_buf());
    handler
        .apply(&directive(
            "record.start",
            sv + 1,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .expect("record.start on a RUNNING show with encoder must open the pipeline");
    state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .expect("session must be present")
        .output_path()
        .to_path_buf()
}

/// Feed `n` full-View frames through the REAL loop feed fns (pre-check +
/// handoff), pushing `tone_s` of tone into the shared tap first.
fn feed_take(state: &Arc<EngineState>, n: u32, tone_s: u32) {
    use nbe_engine::render::{VIEW_H, VIEW_W};
    let tap = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .expect("session must be present")
        .tap();
    tap.push(&tone_secs(tone_s));
    let budget = Duration::from_secs_f64(1.0 / 30.0);
    for frame in 0..n {
        assert!(!should_skip_record_frame(
            Duration::from_millis(2),
            Some(budget)
        ));
        let started = Instant::now();
        let rgba = synthetic_rgba(VIEW_W, VIEW_H, frame);
        let readback_elapsed = started.elapsed();
        let tx = state
            .record_session
            .lock()
            .unwrap()
            .as_ref()
            .expect("session must be present")
            .frame_sender();
        let outcome = handoff_record_frame(rgba, &tx, readback_elapsed);
        assert!(outcome.sent, "a live thread must accept handoffs");
        *state.record_tap_ms.lock().unwrap() += outcome.feed_ms;
    }
}

#[tokio::test]
async fn record_start_on_running_show_opens_pipeline() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, outgoing) = harness();
    let video_path = start_recording(&state, &handler, 1).await;

    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Recording);
    assert!(
        state.record_session.lock().unwrap().is_some(),
        "a record session must be present after start"
    );
    assert!(
        state.record_tap.lock().unwrap().is_some(),
        "start must publish the shared tap for the audio driver"
    );
    // No package loaded: show falls back, episode comes from `outputId`,
    // timestamp is generated — the shape, not the values, is the contract.
    let name = video_path.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("show_ep01_") && name.ends_with(".mp4"),
        "filename derives show/episode/timestamp, got {name}"
    );
    assert_eq!(state.last_applied(), 2);
    assert!(
        acked(&outgoing, 2),
        "start must be acked after the pipeline opened"
    );
    // Leave no thread behind (no stop issued in this test).
    if let Some(mut s) = state.record_session.lock().unwrap().take() {
        s.abandon();
    }
    *state.record_state.lock().unwrap() = RecordState::Idle;
    *state.record_tap.lock().unwrap() = None;
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn record_start_without_running_show_is_forbidden_and_stores_nothing() {
    let _serial = SERIAL.lock().await;
    let (state, handler, _) = harness();
    assert!(!state.is_running());

    let err = handler
        .apply(&directive(
            "record.start",
            1,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .expect_err("record.start with no RUNNING show must be refused");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(state.record_session.lock().unwrap().is_none());
}

#[tokio::test]
async fn record_start_with_failed_encoder_probe_is_refused() {
    // SPEC §16.14 precondition, wired (no dead variant): the start path probes
    // for a hardware encoder WITHOUT opening a stream. Forced-unavailable
    // behaves exactly like missing hardware: refused, loudly, with no session
    // and no ack.
    let _serial = SERIAL.lock().await;
    let _force = ForceNoEncoderGuard::set();
    let (state, handler, outgoing) = harness();
    handler
        .apply(&directive(
            "show.start",
            1,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    *state.record_dir.lock().unwrap() = Some(dir.path().to_path_buf());

    let err = handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .expect_err("start with no encoder must be refused");

    assert!(
        err.to_string().contains("E_NO_HARDWARE_ENCODER"),
        "expected E_NO_HARDWARE_ENCODER, got: {err}"
    );
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(state.record_session.lock().unwrap().is_none());
    assert!(
        state.record_tap.lock().unwrap().is_none(),
        "refused start publishes no tap"
    );
    assert!(!acked(&outgoing, 2), "refused start must not ack");
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn second_record_start_while_recording_is_forbidden_and_preserves_pipeline() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, _) = harness();
    let video_path = start_recording(&state, &handler, 1).await;
    handler
        .apply(&directive(
            "marker.add",
            3,
            serde_json::json!({}),
            serde_json::json!({"name": "keep-me"}),
        ))
        .await
        .unwrap();
    assert_eq!(nbe_engine::record::markers::list().len(), 1);

    let err = handler
        .apply(&directive(
            "record.start",
            4,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .expect_err("second record.start while Recording must be refused");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Recording,
        "refused second start must leave the recording running"
    );
    assert_eq!(
        state
            .record_session
            .lock()
            .unwrap()
            .as_ref()
            .expect("pipeline preserved")
            .output_path(),
        video_path.as_path(),
        "refused second start must not replace the pipeline"
    );
    assert!(
        nbe_engine::record::markers::list().is_empty(),
        "refused second start resets chapters for the next take"
    );
    // The preserved pipeline still takes frames after the refusal.
    feed_take(&state, 5, 1);
    if let Some(mut s) = state.record_session.lock().unwrap().take() {
        s.abandon();
    }
    *state.record_state.lock().unwrap() = RecordState::Idle;
    *state.record_tap.lock().unwrap() = None;
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn record_stop_finishes_file_synchronously_then_acks() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, outgoing) = harness();
    let video_path = start_recording(&state, &handler, 1).await;
    handler
        .apply(&directive(
            "marker.add",
            3,
            serde_json::json!({}),
            serde_json::json!({"name": "chapter-1"}),
        ))
        .await
        .unwrap();
    feed_take(&state, 40, 2);

    handler
        .apply(&directive(
            "record.stop",
            4,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect("record.stop on an active recording must finish the file");

    // Ordering proof: finish ran BEFORE the ack became observable — the file
    // is complete (parses) at the moment last_applied advanced to the stop.
    assert_eq!(state.last_applied(), 4);
    assert!(acked(&outgoing, 4), "stop must be acked after finish ran");
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(state.record_session.lock().unwrap().is_none());
    assert!(
        state.record_tap.lock().unwrap().is_none(),
        "stop detaches the tap from the driver"
    );
    if let Some(ffprobe) = ffprobe_or_skip() {
        let v = ffprobe_streams(&ffprobe, &video_path);
        let streams = v["streams"].as_array().expect("streams array");
        assert_eq!(streams.len(), 2, "exactly 1 video + 1 audio stream");
        assert!(streams.iter().any(|s| s["codec_name"] == "h264"));
        assert!(streams.iter().any(|s| s["codec_name"] == "aac"));
    } else {
        assert_moov_present(&video_path);
    }
    let sidecar = nbe_engine::record::markers::sidecar_path(&video_path);
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).expect("sidecar must exist"))
            .expect("sidecar JSON must parse");
    let entries = parsed["markers"].as_array().expect("markers array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "chapter-1");
    assert!(
        nbe_engine::record::markers::list().is_empty(),
        "graceful stop clears the marker store for the next take"
    );
}

#[tokio::test]
async fn record_stop_failure_withholds_ack_and_keeps_file() {
    // The old defect was `show.stop` swallowing finish errors yet acking. At
    // pipeline level: sabotage the sidecar (a directory where the file should
    // be) so finish fails E_DISK AFTER fragments flushed — the stop surfaces
    // the error (no ack) and the file stays as-is (still parseable: fragments
    // were flushed mid-take, no finalization required).
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
    let (state, handler, outgoing) = harness();
    let video_path = start_recording(&state, &handler, 1).await;
    feed_take(&state, 40, 2);
    std::fs::create_dir(nbe_engine::record::markers::sidecar_path(&video_path))
        .expect("sidecar sabotage must succeed");

    let err = handler
        .apply(&directive(
            "record.stop",
            4,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect_err("a failed finish must surface, never ack");

    assert!(
        err.to_string().contains("E_DISK"),
        "expected E_DISK, got: {err}"
    );
    assert_ne!(
        state.last_applied(),
        4,
        "failed stop must not advance applied"
    );
    assert!(!acked(&outgoing, 4), "failed stop must not ack");
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(state.record_session.lock().unwrap().is_none());
    // File kept as-is: prior fragments parse without any finalization.
    let v = ffprobe_streams(&ffprobe, &video_path);
    let streams = v["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2, "kept file retains both streams");
    // Sabotage cleanup for the leaked tempdir's siblings (file assertions done).
    std::fs::remove_dir(nbe_engine::record::markers::sidecar_path(&video_path)).ok();
    nbe_engine::record::markers::clear();
}

fn fake_unit(pts_seconds: f64, keyframe: bool) -> nbe_engine::encode::EncodedUnit {
    nbe_engine::encode::EncodedUnit {
        data: if keyframe {
            vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x11, 0x22, 0x33]
        } else {
            vec![0x00, 0x00, 0x00, 0x04, 0x41, 0x11, 0x22, 0x33]
        },
        pts_seconds,
        is_keyframe: keyframe,
    }
}

#[tokio::test]
async fn record_stop_with_synthetic_units_finalizes_file_and_sidecar() {
    // No hardware video needed: synthetic units through the REAL thread prove
    // the stop-finish glue (open → feed → finish → ack) hermetically.
    let _serial = SERIAL.lock().await;
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, outgoing) = harness();
    handler
        .apply(&directive(
            "show.start",
            1,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let tap = Arc::new(nbe_engine::record::AudioTap::new());
    tap.push(&tone_secs(1));
    let session = session_glue::RecordSession::open_with_sets(
        dir.path(),
        "demoshow",
        "ep01",
        "20260914T120000Z",
        640,
        360,
        30,
        tap.clone(),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        (vec![0x67, 0x64, 0x00, 0x1E, 0xAA], vec![0x68, 0x11, 0x22]),
    )
    .expect("test-seam open must succeed where AAC exists");
    let video_path = session.output_path().to_path_buf();
    *state.record_tap.lock().unwrap() = Some(tap);
    *state.record_session.lock().unwrap() = Some(session);
    *state.record_state.lock().unwrap() = RecordState::Recording;

    let tx = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .frame_sender();
    for n in 0..45 {
        tx.send(nbe_engine::record::thread::RecordMsg::Unit(fake_unit(
            n as f64 / 30.0,
            n == 0,
        )))
        .expect("channel must accept units");
    }

    handler
        .apply(&directive(
            "record.stop",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect("record.stop must finish the synthetic session");

    assert_eq!(state.last_applied(), 2);
    assert!(acked(&outgoing, 2));
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert_moov_present(&video_path);
    assert!(
        nbe_engine::record::markers::sidecar_path(&video_path).is_file(),
        "finish must write the always-sidecar"
    );
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn show_stop_on_recording_show_leaves_playable_file() {
    // Quiescence: `show.stop` alone finalizes a still-Recording take (graceful
    // path: internal `record.stop`, bounded wait) and acks the STOP.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, outgoing) = harness();
    let video_path = start_recording(&state, &handler, 1).await;
    feed_take(&state, 40, 2);

    // No record.stop: show.stop alone must quiesce the recording.
    handler
        .apply(&directive(
            "show.stop",
            3,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect("show.stop must succeed while recording");

    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(state.record_session.lock().unwrap().is_none());
    assert_eq!(
        state.last_applied(),
        3,
        "ack stateVersion for the stop must be the stop directive's version"
    );
    assert!(acked(&outgoing, 3));
    if let Some(ffprobe) = ffprobe_or_skip() {
        let v = ffprobe_streams(&ffprobe, &video_path);
        let streams = v["streams"].as_array().expect("streams array");
        assert_eq!(streams.len(), 2, "quiesced file keeps both streams");
        assert!(streams.iter().any(|s| s["codec_name"] == "h264"));
        assert!(streams.iter().any(|s| s["codec_name"] == "aac"));
    } else {
        assert_moov_present(&video_path);
    }
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn show_stop_force_abandons_take_immediately() {
    // §16.1 table (`quiesceOutputs=true, force=true`): immediate stop, warning
    // logged — the file is kept as-is with no finish, the show still stops,
    // and the stop acks (force was requested).
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, outgoing) = harness();
    let video_path = start_recording(&state, &handler, 1).await;
    feed_take(&state, 40, 2);

    handler
        .apply(&directive(
            "show.stop",
            3,
            serde_json::json!({}),
            serde_json::json!({"quiesceOutputs": true, "force": true}),
        ))
        .await
        .expect("forced show.stop must stop immediately");

    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(state.record_session.lock().unwrap().is_none());
    assert!(
        !state.is_running(),
        "forced stop still stops the show clock"
    );
    assert!(acked(&outgoing, 3));
    // Fragments flushed mid-take survive the abandon (or the file never
    // materialized if the thread never processed a frame — either is "as-is").
    if video_path.is_file() {
        let top = top_level_boxes(&std::fs::read(&video_path).unwrap());
        assert_eq!(top.first().map(String::as_str), Some("ftyp"));
    }
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn show_stop_with_quiesce_outputs_false_is_refused_while_recording() {
    // §16.1 table (`quiesceOutputs=false, force=false`): fail with
    // `E_FORBIDDEN_STATE` — the take is NOT finalized, the show keeps running.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, outgoing) = harness();
    start_recording(&state, &handler, 1).await;

    let err = handler
        .apply(&directive(
            "show.stop",
            3,
            serde_json::json!({}),
            serde_json::json!({"quiesceOutputs": false}),
        ))
        .await
        .expect_err("show.stop must not quiesce when told not to");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Recording,
        "refused quiesce must leave the take running"
    );
    assert!(
        state.record_session.lock().unwrap().is_some(),
        "refused quiesce must not consume the pipeline"
    );
    assert!(state.is_running(), "refused stop keeps the show clock");
    assert!(!acked(&outgoing, 3), "refused stop must not ack");
    // Cleanup: the take is still live.
    if let Some(mut s) = state.record_session.lock().unwrap().take() {
        s.abandon();
    }
    *state.record_state.lock().unwrap() = RecordState::Idle;
    *state.record_tap.lock().unwrap() = None;
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn show_stop_with_quiesce_outputs_false_and_force_stops_immediately() {
    // §16.1 table (`quiesceOutputs=false, force=true`): immediate stop — the
    // take is abandoned, the show stops, the stop acks.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, outgoing) = harness();
    start_recording(&state, &handler, 1).await;

    handler
        .apply(&directive(
            "show.stop",
            3,
            serde_json::json!({}),
            serde_json::json!({"quiesceOutputs": false, "force": true}),
        ))
        .await
        .expect("forced stop without quiesce must stop immediately");

    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(!state.is_running());
    assert!(acked(&outgoing, 3));
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn record_start_derives_show_name_from_loaded_package() {
    // Honest derivation: the show component comes from the loaded package's
    // manifest (`show.title`), never from directive payload fields.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, _) = harness();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("media")).unwrap();
    std::fs::write(dir.path().join("media/fallback.png"), b"not-a-png").unwrap();
    std::fs::write(
        dir.path().join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "Dress Rehearsal",
                "video": {"width":1920,"height":1080,"frameRate":30,"colorSpace":"rec709"},
                "audio": {"sampleRate":48000,"loudnessTargetLufs":-16,"truePeakDbtp":-1.5},
                "fallbackAssetId": "fallback"
            },
            "assets": [{"id":"fallback","kind":"image","source":"media/fallback.png"}],
            "scenes": [{"id":"SCN_A1","elements":[]}],
            "rundown": {"id":"R","items":[{"id":"A1","kind":"sceneRef","sceneRef":"SCN_A1"}]},
            "control": {"bindings":[]}
        })
        .to_string(),
    )
    .unwrap();
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .expect("show.load must succeed");
    handler
        .apply(&directive(
            "show.start",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let recdir = tempfile::tempdir().unwrap();
    *state.record_dir.lock().unwrap() = Some(recdir.path().to_path_buf());
    handler
        .apply(&directive(
            "record.start",
            3,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .expect("record.start must open");
    let name = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .output_path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        name.starts_with("Dress_Rehearsal_ep01_"),
        "show component derives from the package manifest, got {name}"
    );
    if let Some(mut s) = state.record_session.lock().unwrap().take() {
        s.abandon();
    }
    *state.record_state.lock().unwrap() = RecordState::Idle;
    *state.record_tap.lock().unwrap() = None;
    nbe_engine::record::markers::clear();
}
