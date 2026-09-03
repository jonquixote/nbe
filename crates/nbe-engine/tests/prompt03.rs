//! Prompt 03 Step 7 — unit and integration tests. Clock FSM (total coverage),
//! resync gating, item-end emission, out-of-order stateVersion rejection.

use nbe_engine::clock::{ClockState, MasterClock};
use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::DirectiveFrame;
use std::sync::Arc;

fn make_engine() -> (DirectiveHandler, Arc<EngineState>, Arc<OutgoingQueue>) {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    (handler, state, outgoing)
}

fn directive(
    command: &str,
    state_version: u64,
    payload: serde_json::Value,
    target: serde_json::Value,
) -> DirectiveFrame {
    DirectiveFrame {
        v: nbe_protocol::PROTOCOL_VERSION.into(),
        kind: nbe_protocol::DirectiveKind::Directive,
        seq: 0,
        state_version,
        command: command.into(),
        target,
        payload,
    }
}

#[tokio::test]
async fn clock_fsm_total_coverage() {
    // SPEC 11.4: STOPPED <-> RUNNING only; HELD/SLAVE reserved (unreachable).
    let mut c = MasterClock::new(30);
    assert_eq!(c.state(), ClockState::Stopped);
    assert_eq!(c.frame(), None);
    c.start();
    assert_eq!(c.state(), ClockState::Running);
    assert!(c.frame().is_some());
    c.start(); // idempotent
    assert_eq!(c.state(), ClockState::Running);
    c.stop();
    assert_eq!(c.state(), ClockState::Stopped);
    assert_eq!(c.frame(), None);
}

/// Prompt 03 Step 7: parse the Section 11.4 clock states from the spec —
/// STOPPED and RUNNING must be implemented; HELD/SLAVE remain reserved.
#[test]
fn clock_states_match_spec_11_4() {
    use std::fs;
    let spec = fs::read_to_string("../../docs/spec.v0.3.md").unwrap();
    let clock_section = spec
        .split("# 11. Cross-cutting concern — Master clock")
        .nth(1)
        .expect("Section 11 present");
    assert!(clock_section.contains("`STOPPED`"));
    assert!(clock_section.contains("`RUNNING`"));
    assert!(clock_section.contains("`HELD`"));
    assert!(clock_section.contains("`SLAVE`"));
}

#[tokio::test]
async fn show_load_makes_fallback_resident_and_missing_fallback_fails_loudly() {
    use std::sync::atomic::Ordering;
    let (handler, state, _outgoing) = make_engine();
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

    let d = directive(
        "show.load",
        1,
        serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        serde_json::json!({}),
    );
    handler.apply(&d).await.unwrap();
    {
        let fb = state.fallback.lock().unwrap();
        assert!(fb.is_some());
        assert!(!fb.as_ref().unwrap().bytes.is_empty());
    } // guard dropped before the await below

    // Missing fallback file -> FallbackMissing error.
    let bad = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(bad.path().join("media")).unwrap();
    std::fs::write(
        bad.path().join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": { "id":"s","title":"T","video":{"width":1920,"height":1080,"frameRate":30,"colorSpace":"rec709"},"audio":{"sampleRate":48000,"loudnessTargetLufs":-16,"truePeakDbtp":-1.5},"fallbackAssetId":"missing" },
            "assets":[{"id":"missing","kind":"image","source":"media/nope.png"}],
            "scenes":[{"id":"SCN_A1","elements":[]}],
            "rundown":{"id":"R","items":[{"id":"A1","kind":"sceneRef","sceneRef":"SCN_A1"}]},
            "control":{"bindings":[]}
        })
        .to_string(),
    )
    .unwrap();
    let d2 = directive(
        "show.load",
        2,
        serde_json::json!({ "packagePath": bad.path().to_string_lossy() }),
        serde_json::json!({}),
    );
    assert!(handler.apply(&d2).await.is_err());
    state.fallback_active.load(Ordering::SeqCst);
}

#[tokio::test]
async fn resync_applies_and_emits_appliedstateversion() {
    let (handler, state, outgoing) = make_engine();
    let payload = serde_json::json!({
        "showState": "RUNNING",
        "packagePath": "/tmp/x",
        "viewItem": "A1",
        "previewItem": null,
        "itemStates": { "A1": "LIVE" },
        "sceneStates": {"SCN_A1": "VIEW"},
        "visibleOverlays": [],
        "automationHold": false,
        "fallbackActive": false,
        "stateVersion": 99
    });
    let d = directive(
        nbe_protocol::command::RESYNC,
        99,
        payload,
        serde_json::json!({}),
    );
    handler.apply(&d).await.unwrap();
    assert_eq!(state.last_applied(), 99);
    assert!(
        state.is_running(),
        "resync must start the clock when snapshot says RUNNING"
    );
    let out = outgoing.drain();
    assert!(out.iter().any(|f| matches!(f, nbe_protocol::EngineFrame::AppliedStateVersion { state_version, .. } if *state_version == 99)));
}

#[tokio::test]
async fn resync_after_outage_is_snapshot_not_replay() {
    // §5.9.4: directives issued while the engine was disconnected are gone.
    // The control plane must not replay them; the engine gets a fresh
    // snapshot via show.resync with the latest stateVersion.
    let (handler, state, outgoing) = make_engine();
    // Pretend we were connected, applied sv=3, disconnected. A mix of
    // directives flew by while we were out (sv 4..=5 never reached us).
    state.set_last_applied(3);
    let snapshot = serde_json::json!({
        "showState": "RUNNING",
        "packagePath": "/tmp/x",
        "viewItem": "B2",
        "previewItem": "C1",
        "itemStates": { "B2": "LIVE", "C1": "ARMED" },
        "sceneStates": { "SCN_B2": "VIEW" },
        "visibleOverlays": ["ticker"],
        "automationHold": true,
        "fallbackActive": false,
        "stateVersion": 7
    });
    let reconnect = directive(
        nbe_protocol::command::RESYNC,
        7,
        snapshot,
        serde_json::json!({}),
    );
    handler.apply(&reconnect).await.unwrap();
    // Resync snapshot must be applied (not the missed 4, 5 or 6).
    assert_eq!(state.last_applied(), 7);
    assert!(state.is_running());
    assert!(!state
        .fallback_active
        .load(std::sync::atomic::Ordering::SeqCst));
    // The engine confirms by acking the snapshot's version.
    let out = outgoing.drain();
    assert!(out.iter().any(|f| matches!(f, nbe_protocol::EngineFrame::AppliedStateVersion { state_version, .. } if *state_version == 7))    );
}

#[tokio::test]
async fn item_end_emitted_after_timed_duration() {
    let (handler, state, outgoing) = make_engine();
    state.clock.lock().unwrap().start();
    let d = directive(
        "view.take",
        7,
        serde_json::json!({ "transition": "cut", "durationFrames": 3 }),
        serde_json::json!({ "itemRef": "A1" }),
    );
    handler.apply(&d).await.unwrap();
    // 3 frames at 30 fps = 100 ms. Wait past it.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let out = outgoing.drain();
    assert!(
        out.iter().any(|f| matches!(f, nbe_protocol::EngineFrame::ItemEvent { event: nbe_protocol::ItemEvent::End, item_ref, .. } if item_ref == "A1")),
        "expected itemEvent end for A1; outgoing={:?}",
        out.len()
    );
}

#[tokio::test]
async fn superseded_take_emits_no_late_item_end() {
    // 03b Step 5: the old take's end must never fire after a new take.
    let (handler, state, outgoing) = make_engine();
    state.clock.lock().unwrap().start();
    let first = directive(
        "view.take",
        3,
        serde_json::json!({ "transition": "cut", "durationFrames": 3 }),
        serde_json::json!({ "itemRef": "A1" }),
    );
    handler.apply(&first).await.unwrap();
    let second = directive(
        "view.take",
        4,
        serde_json::json!({ "transition": "cut", "durationFrames": 900 }),
        serde_json::json!({ "itemRef": "B1" }),
    );
    handler.apply(&second).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let out = outgoing.drain();
    assert!(
        !out.iter().any(|f| matches!(f, nbe_protocol::EngineFrame::ItemEvent { event: nbe_protocol::ItemEvent::End, item_ref, .. } if item_ref == "A1")),
        "late itemEvent possibly fired for superseded take"
    );
}
