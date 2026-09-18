//! Peer-review ACCEPTED items (P9-recording): regression pins for the
//! eleven fixes. TDD RED-first: this file references the new seams and
//! asserts the new behavior, so it fails before the fixes land.

use std::sync::Arc;
use std::time::Duration;

use nbe_engine::directive::{DirectiveError, DirectiveHandler};
use nbe_engine::record::markers;
use nbe_engine::record::writer::{MAX_BUFFERED_AUDIO_PACKETS, MAX_BUFFERED_VIDEO_FRAMES};
use nbe_engine::record::{RecordingWriter, RECORD_STOP_TIMEOUT};
use nbe_engine::state::{EngineState, OutgoingQueue, RecordState};
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

fn harness() -> (Arc<EngineState>, DirectiveHandler) {
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    (state, handler)
}

/// The review tests share the process-wide marker store, so they serialize.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Abandon the open session (if any) and wait until its thread-side marker
/// clear has landed, so a detached record thread cannot clear the store
/// mid-way through the NEXT test holding SERIAL. Deterministic: the sentinel
/// is added before the abandon, and the abandon path always clears — provided
/// the frame senders stay alive until the clear lands (a dropped session
/// routes the thread down the frames-gone path, which honors a queued `Stop`
/// but not `Abandon`). Hence `held` stays alive through the poll.
async fn abandon_and_drain(state: &Arc<EngineState>) {
    let held = state.record_session.lock().unwrap().take();
    markers::add(markers::Marker {
        name: "__drain__".into(),
        frame: u64::MAX,
        timecode: None,
    });
    if let Some(mut s) = held {
        s.abandon();
        for _ in 0..200 {
            if !markers::list().iter().any(|m| m.name == "__drain__") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            !markers::list().iter().any(|m| m.name == "__drain__"),
            "detached abandon must clear the store promptly"
        );
        drop(s);
    }
    *state.record_state.lock().unwrap() = RecordState::Idle;
    *state.record_tap.lock().unwrap() = None;
    markers::clear();
}

fn aac_or_skip() -> bool {
    if nbe_engine::record::aac::is_available() {
        true
    } else {
        eprintln!("SKIP: AudioToolbox AAC absent");
        false
    }
}

fn hw_or_skip() -> bool {
    if nbe_engine::encode::is_available() {
        return true;
    }
    eprintln!("SKIP: no hardware H.264 encoder on this machine (SPEC §9.2); recording has no CPU fallback");
    false
}

fn test_params(dir: &std::path::Path) -> nbe_engine::record::RecordParams {
    nbe_engine::record::RecordParams {
        directory: dir.to_path_buf(),
        show: "demoshow".into(),
        episode: "ep01".into(),
        start_timestamp: "20260914T120000Z".into(),
        width: 640,
        height: 360,
        fps: 30,
        sps: vec![0x67, 0x64, 0x00, 0x1E, 0xAA],
        pps: vec![0x68, 0x11, 0x22],
    }
}

fn unit(pts: f64, keyframe: bool) -> nbe_engine::encode::EncodedUnit {
    nbe_engine::encode::EncodedUnit {
        data: if keyframe {
            vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x11, 0x22, 0x33]
        } else {
            vec![0x00, 0x00, 0x00, 0x04, 0x41, 0x11, 0x22, 0x33]
        },
        pts_seconds: pts,
        is_keyframe: keyframe,
    }
}

// Item 2: 2000ms -> 1500ms headroom inside the SPEC §16.1 2s window.
#[test]
fn stop_timeout_leaves_headroom_inside_spec_window() {
    assert_eq!(
        RECORD_STOP_TIMEOUT,
        Duration::from_millis(1500),
        "RECORD_STOP_TIMEOUT must be 1500ms (ack pump + WS need the headroom)"
    );
}

// Item 1: writer window accumulation is bounded, sheds oldest, counts.
#[test]
fn writer_window_accumulation_is_bounded_and_sheds_counted() {
    if !aac_or_skip() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let p = test_params(dir.path());
    let mut w = RecordingWriter::create(&p).expect("create must succeed");
    w.set_parameter_sets(p.sps.clone(), p.pps.clone())
        .expect("sets must install before first IDR");
    // No audio is ever pushed, so no window can complete: pre-fix this
    // buffers all 900 units (~30s) unbounded.
    for n in 0..900 {
        w.push_video(&unit(n as f64 / 30.0, n == 0)).unwrap();
    }
    assert!(
        w.buffered_video_len() <= MAX_BUFFERED_VIDEO_FRAMES,
        "video window must be capped at {MAX_BUFFERED_VIDEO_FRAMES}, held {}",
        w.buffered_video_len()
    );
    assert!(
        w.dropped_video() > 0,
        "shed video must be counted (drop-oldest-counted)"
    );

    // Audio side: packets with no video window to join accumulate too.
    let mut w2 = RecordingWriter::create(&test_params(dir.path())).expect("create must succeed");
    let tone: Vec<f32> = (0..48_000 * 8)
        .flat_map(|n| {
            let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48_000.0).sin();
            [v, v]
        })
        .collect();
    w2.push_audio(&tone).unwrap();
    assert!(
        w2.buffered_audio_packets() <= MAX_BUFFERED_AUDIO_PACKETS,
        "audio window must be capped at {MAX_BUFFERED_AUDIO_PACKETS}, held {}",
        w2.buffered_audio_packets()
    );
    assert!(
        w2.dropped_audio_packets() > 0,
        "shed audio must be counted (drop-oldest-counted)"
    );
}

// Item 3: stop -> restart in the same second must not truncate.
#[tokio::test]
async fn rapid_restart_yields_distinct_files() {
    if !hw_or_skip() {
        return;
    }
    let _serial = SERIAL.lock().await;
    let (state, handler) = harness();
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
            serde_json::json!({}),
        ))
        .await
        .expect("first start must open");
    let first = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .output_path()
        .to_path_buf();
    // Empty take: loud stop, state back to Idle (no file materializes).
    let _ = handler
        .apply(&directive(
            "record.stop",
            3,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await;
    handler
        .apply(&directive(
            "record.start",
            4,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect("second start must open");
    let second = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .output_path()
        .to_path_buf();
    assert_ne!(
        first, second,
        "two rapid takes must reserve distinct files (no truncate), got {first:?} twice"
    );
    abandon_and_drain(&state).await;
}

// Item 4: marker STORE count cap, documented; finish serializes bounded list.
#[tokio::test]
async fn marker_store_caps_at_documented_bound() {
    let _serial = SERIAL.lock().await;
    markers::clear();
    for n in 0..(markers::MAX_MARKERS + 100) {
        markers::add(markers::Marker {
            name: format!("m{n}"),
            frame: n as u64,
            timecode: None,
        });
    }
    let list = markers::list();
    assert_eq!(
        list.len(),
        markers::MAX_MARKERS,
        "store must cap at MAX_MARKERS ({}), held {}",
        markers::MAX_MARKERS,
        list.len()
    );
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let video = dir.path().join("take.mp4");
    std::fs::write(&video, b"fake").unwrap();
    markers::write_sidecar(&video, &list).expect("sidecar must write");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(markers::sidecar_path(&video)).unwrap())
            .unwrap();
    assert_eq!(
        parsed["markers"].as_array().unwrap().len(),
        markers::MAX_MARKERS,
        "finish/sidecar must serialize the bounded list"
    );
    markers::clear();
}

// Item 6: admission checks space early -> E_DISK on unwritable target.
#[tokio::test]
async fn record_start_refuses_unwritable_target_early_with_e_disk() {
    let _serial = SERIAL.lock().await;
    let (state, handler) = harness();
    handler
        .apply(&directive(
            "show.start",
            1,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let ro = tempfile::tempdir().expect("tempdir must succeed");
    let mut perms = std::fs::metadata(ro.path()).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o555);
    }
    #[cfg(not(unix))]
    {
        perms.set_readonly(true);
    }
    std::fs::set_permissions(ro.path(), perms).unwrap();
    if nbe_engine::record::available_space_mib(ro.path()).is_ok() {
        eprintln!("SKIP: read-only dir is writable here (root?) — admission untestable");
        return;
    }
    *state.record_dir.lock().unwrap() = Some(ro.path().to_path_buf());
    let err = handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect_err("start into an unwritable target must be refused early");
    assert!(
        err.to_string().contains("E_DISK"),
        "expected E_DISK, got: {err}"
    );
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(state.record_session.lock().unwrap().is_none());
    markers::clear();
}

// Item 7: reset record_tap_ms + skipped_record_frames on record.start.
#[tokio::test]
async fn record_start_resets_counters() {
    if !hw_or_skip() {
        return;
    }
    let _serial = SERIAL.lock().await;
    let (state, handler) = harness();
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
    *state.record_tap_ms.lock().unwrap() = 1234.5;
    state
        .skipped_record_frames
        .store(999, std::sync::atomic::Ordering::SeqCst);
    handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect("start must open");
    assert_eq!(
        *state.record_tap_ms.lock().unwrap(),
        0.0,
        "record_tap_ms must reset on start"
    );
    assert_eq!(
        state
            .skipped_record_frames
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "skipped_record_frames must reset on start"
    );
    abandon_and_drain(&state).await;
}

// Item 8: await_done Disconnected maps to Timeout, not Input.
#[test]
fn await_done_disconnected_maps_to_timeout() {
    let (tx, rx) = std::sync::mpsc::channel::<
        Result<std::path::PathBuf, nbe_engine::record::session::SessionError>,
    >();
    drop(tx);
    let err = nbe_engine::record::await_done(&rx, Duration::from_secs(5)).unwrap_err();
    assert!(
        matches!(err, nbe_engine::record::session::SessionError::Timeout(_)),
        "dead thread with no report must be Timeout, got: {err}"
    );
    assert!(
        err.to_string().contains("E_RECORD_TIMEOUT"),
        "timeout token must be E_RECORD_TIMEOUT, got: {err}"
    );
    assert!(
        err.to_string().contains("kept as-is"),
        "timeout must promise the file is kept, got: {err}"
    );
}

// Item 9: second stop with no session must not clear markers.
#[tokio::test]
async fn sessionless_stop_preserves_markers() {
    let _serial = SERIAL.lock().await;
    markers::clear();
    let (state, handler) = harness();
    handler
        .apply(&directive(
            "show.start",
            1,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    *state.record_state.lock().unwrap() = RecordState::Recording;
    assert!(state.record_session.lock().unwrap().is_none());
    handler
        .apply(&directive(
            "marker.add",
            2,
            serde_json::json!({}),
            serde_json::json!({"name": "keep-me"}),
        ))
        .await
        .unwrap();
    handler
        .apply(&directive(
            "record.stop",
            3,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect("session-less stop must succeed");
    assert_eq!(
        markers::list().len(),
        1,
        "only the session owner clears markers; a session-less stop must preserve them"
    );
    markers::clear();
}

// Item 10: resolution guard refuses above 1920x1080 with E_UNSUPPORTED.
#[test]
fn resolution_guard_refuses_above_1080p() {
    assert!(
        nbe_engine::directive::check_record_resolution(1920, 1080).is_ok(),
        "at-cap 1920x1080 must record"
    );
    assert!(
        nbe_engine::directive::check_record_resolution(1280, 720).is_ok(),
        "below-cap must record"
    );
    for (w, h) in [(3840, 2160), (1921, 1080), (1920, 1081), (4096, 2160)] {
        let err = nbe_engine::directive::check_record_resolution(w, h)
            .expect_err("above-cap geometry must be refused");
        assert!(
            matches!(err, DirectiveError::Unsupported(_)),
            "expected E_UNSUPPORTED variant, got: {err}"
        );
        assert!(
            err.to_string().contains("E_UNSUPPORTED"),
            "expected E_UNSUPPORTED token, got: {err}"
        );
    }
}
