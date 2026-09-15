//! Prompt 09 WU1 (SPEC §16.14): engine `record.start` / `record.stop`
//! precondition validation + `RecordState` skeleton.
//!
//! Design as built (not as first sketched): `record.start` never touches the
//! encoder — it validates preconditions, reserves the output path, and flips
//! to Recording. Encoder absence degrades at feed/finish time (skipped frames,
//! loud finish), it does not refuse the start. So this file pins the refusal
//! paths that remain — `E_FORBIDDEN_STATE` when the show is not RUNNING —
//! plus the start-opens/stop-empty-loud contract. `record.stop` requires an
//! active recording and is refused with `E_FORBIDDEN_STATE` while Idle.

use std::sync::Arc;

use nbe_engine::directive::{DirectiveError, DirectiveHandler};
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

#[test]
fn record_state_defaults_to_idle() {
    assert_eq!(
        *EngineState::new(30).record_state.lock().unwrap(),
        RecordState::Idle
    );
}

#[tokio::test]
async fn record_start_without_running_show_is_forbidden_and_state_unchanged() {
    let (state, handler) = harness();
    assert!(!state.is_running(), "clock starts stopped");

    let err = handler
        .apply(&directive(
            "record.start",
            1,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect_err("record.start with no RUNNING show must be refused");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Idle,
        "refused start must not mutate record state"
    );
}

#[tokio::test]
async fn record_start_on_running_show_opens_and_empty_stop_is_loud() {
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
    assert!(state.is_running(), "show.start must run the clock");

    // Start opens without touching the encoder: dir configured, preconditions
    // met → Ok + Recording + reserved session (path exists as reservation).
    *state.record_dir.lock().unwrap() = Some(std::env::temp_dir());
    handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap_or_else(|e| panic!("start with dir set must open, got: {e}"));
    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Recording,
        "open start must flip to Recording"
    );
    assert!(
        state.record_session.lock().unwrap().is_some(),
        "open start must reserve a session"
    );

    // Stop with no fed frames: loud E_RECORD_INPUT (no video units), state
    // back to Idle, ack withheld (apply returns Err).
    let err = handler
        .apply(&directive(
            "record.stop",
            3,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect_err("empty session finish must be loud");
    assert!(
        err.to_string().contains("E_RECORD_INPUT"),
        "expected E_RECORD_INPUT, got: {err}"
    );
    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Idle,
        "failed finish still ends the recording"
    );
}

#[tokio::test]
async fn record_stop_while_idle_is_forbidden() {
    let (state, handler) = harness();

    let err = handler
        .apply(&directive(
            "record.stop",
            1,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect_err("record.stop with no active recording must be refused");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Idle,
        "refused stop must not mutate record state"
    );
}

#[tokio::test]
async fn record_stop_ends_an_active_recording() {
    // The transition exists for WU2 (the only path that can enter
    // Recording): stop flips Recording -> Idle.
    let (state, handler) = harness();
    *state.record_state.lock().unwrap() = RecordState::Recording;

    handler
        .apply(&directive(
            "record.stop",
            1,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect("record.stop on an active recording must succeed");

    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Idle,
        "stop must return the state machine to Idle"
    );
}

#[tokio::test]
async fn record_start_into_unwritable_target_reports_e_disk() {
    // The E_DISK token must survive from RecordError::Disk through the
    // directive boundary (it once dissolved into a bare io error here).
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
    let blocker = std::env::temp_dir().join("p09-edisk-blocker");
    std::fs::write(&blocker, b"x").unwrap();
    *state.record_dir.lock().unwrap() = Some(blocker.clone());
    let err = handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect_err("record into a file path must fail");
    std::fs::remove_file(&blocker).ok();
    assert!(
        err.to_string().contains("E_DISK"),
        "expected E_DISK token, got: {err}"
    );
    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Idle,
        "refused start must leave the state machine Idle"
    );
}
