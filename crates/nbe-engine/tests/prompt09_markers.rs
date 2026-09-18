//! Prompt 09 WU5 [RI-5] (SPEC §16.11): `marker.add` → recording chapters.
//!
//! TDD: written BEFORE the implementation (RED first). `nbe_engine::record::markers`
//! does not exist yet — this file must fail to compile until `markers.rs` lands,
//! proving the tests exercise new code.
//!
//! Coverage maps to the work-unit DoD:
//! 1. `marker.add` while Recording with name + master-frame → stored
//!    {name, timecode/frame}; sidecar JSON beside the file at finish parses
//!    and its entries match.
//! 2. `marker.add` while Idle → `E_FORBIDDEN_STATE`, nothing stored
//!    (accept-but-ignore is silent loss and is refused instead).
//! 3. `chapters()` exposes the ordered chapter list (the Matroska-chapter API);
//!    the writer only supports fMP4 today, so chapters ride the sidecar —
//!    see `markers.rs` docs for the honest container story.

use std::sync::Arc;

use nbe_engine::directive::{DirectiveError, DirectiveHandler};
use nbe_engine::record::markers::{self, Marker};
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

fn is_forbidden(err: &DirectiveError) -> bool {
    matches!(err, DirectiveError::ForbiddenState(_))
        && err.to_string().contains("E_FORBIDDEN_STATE")
}

/// The marker store is process-wide (see `markers.rs`), so the tests that
/// touch it through the directive path serialize on this lock.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Arm an active recording the way WU2 would: show RUNNING (so the master
/// frame exists) plus `RecordState::Recording` set directly — `record.start`
/// has no encoder path in this work unit, same seam as `prompt09_record.rs`.
async fn arm_recording(state: &Arc<EngineState>, handler: &DirectiveHandler) {
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
}

#[tokio::test]
async fn marker_add_while_recording_stores_name_frame_and_timecode() {
    let _guard = SERIAL.lock().await;
    markers::clear();
    let (state, handler) = harness();
    arm_recording(&state, &handler).await;
    let frame_before = state.master_frame();

    handler
        .apply(&directive(
            "marker.add",
            2,
            serde_json::json!({}),
            serde_json::json!({"name": "segment-b", "timecode": "00:01:00:00"}),
        ))
        .await
        .expect("marker.add while Recording must succeed");

    let stored = markers::list();
    assert_eq!(stored.len(), 1, "exactly one marker must be stored");
    assert_eq!(stored[0].name, "segment-b");
    assert_eq!(
        stored[0].timecode.as_deref(),
        Some("00:01:00:00"),
        "verbatim timecode must survive"
    );
    let frame_after = state.master_frame();
    // Next-frame-boundary discipline (same as take/overlay): the marker
    // records master_frame() + 1, never mid-frame.
    let before_u = frame_before.unwrap_or(0);
    let after_u = frame_after.unwrap_or(u64::MAX);
    assert!(
        stored[0].frame > before_u && stored[0].frame <= after_u + 1,
        "marker frame {} must be master_frame()+1 bracketing {:?}..={:?}",
        stored[0].frame,
        frame_before,
        frame_after
    );
}

#[tokio::test]
async fn marker_sidecar_beside_file_parses_and_matches_stored_markers() {
    let _guard = SERIAL.lock().await;
    markers::clear();
    let (state, handler) = harness();
    arm_recording(&state, &handler).await;

    for (sv, name) in [(2, "intro"), (3, "segment-b")] {
        handler
            .apply(&directive(
                "marker.add",
                sv,
                serde_json::json!({}),
                serde_json::json!({"name": name}),
            ))
            .await
            .unwrap();
    }
    let stored = markers::list();
    assert_eq!(stored.len(), 2);

    // The "finish" shape: sidecar JSON written beside the recording file.
    let dir = tempfile::tempdir().expect("tempdir must exist");
    let video = dir.path().join("show_ep_20260914T120000Z.mp4");
    std::fs::write(&video, b"fake-mp4").unwrap();
    let sidecar = markers::write_sidecar(&video, &stored).expect("sidecar write must succeed");
    assert_eq!(
        sidecar.parent().unwrap(),
        video.parent().unwrap(),
        "sidecar must sit beside the recording file"
    );

    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap())
            .expect("sidecar JSON must parse");
    let entries = parsed
        .get("markers")
        .and_then(|v| v.as_array())
        .expect("sidecar must carry a `markers` array");
    assert_eq!(
        entries.len(),
        2,
        "sidecar must contain the full marker list"
    );
    for (entry, marker) in entries.iter().zip(stored.iter()) {
        assert_eq!(
            entry.get("name").and_then(|v| v.as_str()),
            Some(marker.name.as_str())
        );
        assert_eq!(
            entry.get("frame").and_then(|v| v.as_u64()),
            Some(marker.frame)
        );
    }
}

#[tokio::test]
async fn marker_add_while_idle_is_forbidden_and_stores_nothing() {
    let _guard = SERIAL.lock().await;
    markers::clear();
    let (state, handler) = harness();
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);

    let err = handler
        .apply(&directive(
            "marker.add",
            1,
            serde_json::json!({}),
            serde_json::json!({"name": "lost-chapter"}),
        ))
        .await
        .expect_err("marker.add while Idle must be refused, never silently dropped");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert!(
        markers::list().is_empty(),
        "refused marker must be recorded nowhere"
    );
}

#[test]
fn chapters_accessor_exposes_ordered_chapters() {
    // The Matroska-chapter API: ordered by frame even when added out of order.
    // The writer only supports fMP4 today (no in-container chapters), so these
    // ride the sidecar; `chapters()` is the seam the Matroska muxer will read.
    let unordered = vec![
        Marker {
            name: "b".into(),
            frame: 90,
            timecode: None,
        },
        Marker {
            name: "a".into(),
            frame: 30,
            timecode: Some("00:00:01:00".into()),
        },
        Marker {
            name: "c".into(),
            frame: 150,
            timecode: None,
        },
    ];
    let chapters = markers::chapters(&unordered);
    let names: Vec<&str> = chapters.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c"], "chapters must be frame-ordered");
}

fn test_record_params(dir: &std::path::Path) -> nbe_engine::record::RecordParams {
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

fn keyframe_unit(pts_seconds: f64) -> nbe_engine::encode::EncodedUnit {
    nbe_engine::encode::EncodedUnit {
        data: vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x11, 0x22, 0x33],
        pts_seconds,
        is_keyframe: true,
    }
}

fn aac_or_skip() -> bool {
    if nbe_engine::record::aac::is_available() {
        true
    } else {
        eprintln!("SKIP: AudioToolbox AAC absent — finish/sidecar tests need the encoder");
        false
    }
}

#[tokio::test]
async fn finish_writes_sidecar_with_matching_entries() {
    let _guard = SERIAL.lock().await;
    if !aac_or_skip() {
        return;
    }
    markers::clear();
    markers::add(Marker {
        name: "intro".into(),
        frame: 30,
        timecode: None,
    });
    markers::add(Marker {
        name: "segment-b".into(),
        frame: 90,
        timecode: Some("00:01:00:00".into()),
    });

    let dir = tempfile::tempdir().expect("tempdir must exist");
    let params = test_record_params(dir.path());
    let mut w = nbe_engine::record::RecordingWriter::create(&params).expect("create must succeed");
    w.push_video(&keyframe_unit(0.0)).unwrap();
    w.push_video(&keyframe_unit(1.0 / 30.0)).unwrap();
    let path = w.finish().expect("finish must succeed");

    let sidecar = markers::sidecar_path(&path);
    assert!(
        sidecar.is_file(),
        "finish must write the sidecar beside the file"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap())
            .expect("sidecar JSON must parse");
    let entries = parsed
        .get("markers")
        .and_then(|v| v.as_array())
        .expect("sidecar must carry a `markers` array");
    assert_eq!(
        entries.len(),
        2,
        "sidecar must contain the full marker list"
    );
    assert_eq!(
        entries[0].get("name").and_then(|v| v.as_str()),
        Some("intro")
    );
    assert_eq!(entries[0].get("frame").and_then(|v| v.as_u64()), Some(30));
    assert_eq!(
        entries[1].get("name").and_then(|v| v.as_str()),
        Some("segment-b")
    );
    assert_eq!(entries[1].get("frame").and_then(|v| v.as_u64()), Some(90));
    markers::clear();
}

#[tokio::test]
async fn finish_sidecar_failure_surfaces_e_disk() {
    let _guard = SERIAL.lock().await;
    if !aac_or_skip() {
        return;
    }
    markers::clear();
    markers::add(Marker {
        name: "intro".into(),
        frame: 30,
        timecode: None,
    });

    let dir = tempfile::tempdir().expect("tempdir must exist");
    let params = test_record_params(dir.path());
    let mut w = nbe_engine::record::RecordingWriter::create(&params).expect("create must succeed");
    w.push_video(&keyframe_unit(0.0)).unwrap();
    // A directory where the sidecar file should be: the sidecar write fails
    // deterministically for any uid (no chmod games).
    std::fs::create_dir(markers::sidecar_path(w.path())).unwrap();

    let err = w
        .finish()
        .expect_err("finish must fail when the sidecar is unwritable");
    let msg = err.to_string();
    assert!(
        msg.contains("E_DISK"),
        "error must carry E_DISK, got: {msg}"
    );
    assert!(
        matches!(err, nbe_engine::record::RecordError::Disk(_)),
        "error variant must be RecordError::Disk"
    );
    markers::clear();
}

#[tokio::test]
async fn record_start_clears_markers_for_fresh_take() {
    let _guard = SERIAL.lock().await;
    markers::clear();
    let (state, handler) = harness();
    arm_recording(&state, &handler).await;
    handler
        .apply(&directive(
            "marker.add",
            2,
            serde_json::json!({}),
            serde_json::json!({"name": "stale"}),
        ))
        .await
        .unwrap();
    assert_eq!(markers::list().len(), 1);

    // No encoder path in this work unit, so record.start is refused — but the
    // marker list must already be fresh for the new take.
    let _ = handler
        .apply(&directive(
            "record.start",
            3,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await;
    assert!(
        markers::list().is_empty(),
        "record.start must clear the marker list for a fresh recording"
    );
}

#[tokio::test]
async fn show_load_clears_markers_against_cross_show_pollution() {
    let _guard = SERIAL.lock().await;
    markers::clear();
    let (state, handler) = harness();
    arm_recording(&state, &handler).await;
    handler
        .apply(&directive(
            "marker.add",
            2,
            serde_json::json!({}),
            serde_json::json!({"name": "old-show-chapter"}),
        ))
        .await
        .unwrap();
    assert_eq!(markers::list().len(), 1);

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("media")).unwrap();
    std::fs::write(dir.path().join("media/fallback.png"), b"not-a-png").unwrap();
    std::fs::write(
        dir.path().join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id":"s","title":"T",
                "video":{"width":1920,"height":1080,"frameRate":30,"colorSpace":"rec709"},
                "audio":{"sampleRate":48000,"loudnessTargetLufs":-16,"truePeakDbtp":-1.5},
                "fallbackAssetId":"fallback"
            },
            "assets":[{"id":"fallback","kind":"image","source":"media/fallback.png"}],
            "scenes":[{"id":"SCN_A1","elements":[]}],
            "rundown":{"id":"R","items":[{"id":"A1","kind":"sceneRef","sceneRef":"SCN_A1"}]},
            "control":{"bindings":[]}
        })
        .to_string(),
    )
    .unwrap();
    handler
        .apply(&directive(
            "show.load",
            3,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .expect("show.load must succeed");
    assert!(
        markers::list().is_empty(),
        "show.load must clear the marker list (no cross-show pollution)"
    );
}
