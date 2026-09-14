//! Prompt 09 WU8 (SPEC §16.14, §5.9.5): record session glue — `record.start`
//! opens a session, `record.stop` finishes it synchronously before the ack,
//! `show.stop` quiesces a recording show to a playable file.
//!
//! TDD: written BEFORE the session glue (RED first). `nbe_engine::record::session`
//! does not exist yet — this file must fail to compile until `session.rs` lands,
//! proving the tests exercise new code (same discipline as `prompt09_record_file.rs`).
//!
//! Coverage maps to the work-unit DoD:
//! 1. `record.start` on a RUNNING show with encoder available → `Recording`,
//!    session present, writer in the record directory. Preconditions intact:
//!    not-running → `E_FORBIDDEN_STATE`; forced-unavailable (seam) → `E_NO_HARDWARE_ENCODER`.
//! 2. `record.stop` finishes synchronously (file + sidecar complete) and only
//!    then does the ack flow (`last_applied` + `AppliedStateVersion`).
//!    Quiescence: `show.stop` on a recording show leaves a playable file and
//!    an honest `appliedStateVersion`.
//! 3. Second `record.start` while `Recording` → `E_FORBIDDEN_STATE`, session
//!    preserved, chapters reset for the next take (WU5, documented behavior).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nbe_engine::directive::{DirectiveError, DirectiveHandler};
use nbe_engine::record::session as session_glue;
use nbe_engine::render::{VIEW_H, VIEW_W};
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

fn start_payload() -> serde_json::Value {
    serde_json::json!({
        "show": "demoshow",
        "episode": "ep01",
        "startTimestamp": "20260914T120000Z",
        "width": 640,
        "height": 360,
        "fps": 30,
    })
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

fn hw_or_fail() {
    assert!(
        nbe_engine::encode::is_available(),
        "LOUD FAILURE: no hardware H.264 encoder (SPEC §9.2); recording has no CPU fallback"
    );
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

/// Feed one second of real frames plus its tone into the open session. Frames
/// come from a test-local hardware session at VIEW geometry — the session's
/// own geometry, since `record.start` reserves the path at VIEW_W/H for the
/// loop feed's ONE live encoder — and the encoder's real parameter sets are
/// captured into the session exactly as the loop feed does (the retired
/// premise: a throwaway black-frame capture at a second geometry pre-filled
/// the sets). The encoder tail (finish re-reports the prefix; skip it)
/// follows.
fn feed_one_second(state: &Arc<EngineState>) {
    let mut enc = nbe_engine::encode::EncodeSession::open(VIEW_W, VIEW_H, 30, 8_000_000)
        .expect("EncodeSession::open must succeed where hardware exists");
    // One base frame, cloned per feed: content repeats while PTS advances per
    // frame index — valid stream content at a ninth of the build cost.
    let base = synthetic_rgba(VIEW_W, VIEW_H, 0);
    let mut seen = 0usize;
    let mut guard = state.record_session.lock().unwrap();
    let session = guard.as_mut().expect("session must be present");
    for _ in 0..30 {
        let fresh = enc.encode_rgba(&base).expect("feeding RGBA must succeed");
        seen += fresh.len();
        for u in &fresh {
            session.push_video(u).expect("push_video must succeed");
        }
    }
    // Unification capture: the live encoder's first keyframe exposes the real
    // sets; the session (EMPTY at start) takes them here, as the loop feed
    // does via `set_parameter_sets`.
    let (sps, pps) = enc
        .parameter_sets()
        .expect("a fed keyframe must expose parameter sets");
    session.set_parameter_sets(sps, pps);
    let all = enc
        .finish()
        .expect("encoder finish must complete the stream");
    for u in all.iter().skip(seen) {
        session
            .push_video(u)
            .expect("pushing the encoder tail must succeed");
    }
    session
        .push_audio(&tone_secs(1))
        .expect("push_audio must succeed");
}

#[tokio::test]
async fn record_start_on_running_show_opens_recording_session() {
    let _serial = SERIAL.lock().await;
    hw_or_fail();
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
    *state.record_dir.lock().unwrap() = Some(dir.path().to_path_buf());

    handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .expect("record.start on a RUNNING show with encoder must open a session");

    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Recording);
    assert!(
        state.record_session.lock().unwrap().is_some(),
        "a record session must be present after start"
    );
    assert!(
        state
            .record_session
            .lock()
            .unwrap()
            .as_ref()
            .expect("session present")
            .parameter_sets()
            .is_none(),
        "unification: the session starts metadata-only, sets EMPTY until the loop feed captures them"
    );
    assert_eq!(state.last_applied(), 2);
    assert!(
        acked(&outgoing, 2),
        "start must be acked after the session opened"
    );
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
async fn record_start_with_forced_no_encoder_still_opens_session() {
    // Unification-retired premise: the handler touches NO encoder (the
    // throwaway black-frame capture is deleted), so forced-unavailable no
    // longer refuses at start. The refusal is deferred: the loop feed skips
    // video frames and `finish` without captured sets is E_RECORD_INPUT.
    // Start therefore succeeds with a metadata-only session (EMPTY sets).
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

    handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .expect("start touches no encoder, so forced-unavailable must not refuse it");

    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Recording);
    assert!(
        state
            .record_session
            .lock()
            .unwrap()
            .as_ref()
            .expect("session present")
            .parameter_sets()
            .is_none(),
        "no encoder was touched, so no sets could be captured"
    );
    assert!(acked(&outgoing, 2));
    // Leave no session behind (no stop issued in this test).
    state.record_session.lock().unwrap().take();
    *state.record_state.lock().unwrap() = RecordState::Idle;
    nbe_engine::record::markers::clear();
}

#[tokio::test]
async fn second_record_start_while_recording_is_forbidden_and_preserves_session() {
    let _serial = SERIAL.lock().await;
    hw_or_fail();
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let (state, handler, _) = harness();
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
    handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .unwrap();
    // A chapter of the ONGOING recording: the refused second start resets the
    // chapter list for the next take (WU5) while preserving the live session.
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
    assert!(
        state.record_session.lock().unwrap().is_some(),
        "refused second start must not replace the session"
    );
    assert!(
        nbe_engine::record::markers::list().is_empty(),
        "refused second start resets chapters for the next take (WU5)"
    );
    // Leave the store clean for the binary's other tests.
    nbe_engine::record::markers::clear();
    // Leave no session behind either (no stop issued in this test).
    state.record_session.lock().unwrap().take();
    *state.record_state.lock().unwrap() = RecordState::Idle;
}

#[tokio::test]
async fn record_stop_finishes_file_synchronously_then_acks() {
    let _serial = SERIAL.lock().await;
    hw_or_fail();
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
    *state.record_dir.lock().unwrap() = Some(dir.path().to_path_buf());
    handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .unwrap();
    handler
        .apply(&directive(
            "marker.add",
            3,
            serde_json::json!({}),
            serde_json::json!({"name": "chapter-1"}),
        ))
        .await
        .unwrap();
    feed_one_second(&state);
    let video_path = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.output_path().to_path_buf())
        .expect("session must be present");
    assert_eq!(
        video_path.parent().unwrap(),
        dir.path(),
        "output reserved in the record directory"
    );
    assert_eq!(
        video_path.file_name().unwrap().to_str().unwrap(),
        "demoshow_ep01_20260914T120000Z.mp4"
    );

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
    nbe_engine::record::markers::clear();
}

fn synthetic_params(dir: &Path) -> nbe_engine::record::RecordParams {
    nbe_engine::record::RecordParams {
        directory: dir.to_path_buf(),
        show: "demoshow".into(),
        episode: "ep01".into(),
        start_timestamp: "20260914T120000Z".into(),
        width: 640,
        height: 360,
        fps: 30,
        // Writer-valid dummy sets (cf. prompt09_markers.rs): NAL types 7/8.
        sps: vec![0x67, 0x64, 0x00, 0x1E, 0xAA],
        pps: vec![0x68, 0x11, 0x22],
    }
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
    // No hardware video needed: synthetic units through the writer path prove
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
    let params = synthetic_params(dir.path());
    let session = session_glue::RecordSession::open_synthetic(&params)
        .expect("synthetic open must succeed where AAC exists");
    let video_path = session.output_path().to_path_buf();
    assert_eq!(
        video_path.parent().unwrap(),
        dir.path(),
        "output reserved in the record directory"
    );
    *state.record_session.lock().unwrap() = Some(session);
    *state.record_state.lock().unwrap() = RecordState::Recording;

    {
        let mut guard = state.record_session.lock().unwrap();
        let session = guard.as_mut().unwrap();
        for n in 0..45 {
            session
                .push_video(&fake_unit(n as f64 / 30.0, n == 0))
                .expect("push_video must succeed");
        }
        session
            .push_audio(&tone_secs(1))
            .expect("push_audio must succeed");
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
    // Quiescence: the control plane emits record.stop internally on show.stop,
    // and the engine finalizes a still-Recording session itself — either order
    // leaves a playable file and an honest appliedStateVersion.
    let _serial = SERIAL.lock().await;
    hw_or_fail();
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
    *state.record_dir.lock().unwrap() = Some(dir.path().to_path_buf());
    handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            start_payload(),
        ))
        .await
        .unwrap();
    feed_one_second(&state);
    let video_path = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.output_path().to_path_buf())
        .expect("session must be present");

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
