//! Prompt 09 WU1 (SPEC §16.14): engine `record.start` / `record.stop`
//! precondition validation + `RecordState` skeleton.
//!
//! WU1 scope: no encoder exists yet (that is WU2), so EVERY `record.start`
//! is refused — `E_FORBIDDEN_STATE` when the show is not RUNNING,
//! `E_NO_HARDWARE_ENCODER` when it is. `record.stop` requires an active
//! recording and is refused with `E_FORBIDDEN_STATE` while Idle. The
//! `Idle <-> Recording` transitions exist so WU2 can flip them.

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

fn is_no_encoder(err: &DirectiveError) -> bool {
    matches!(err, DirectiveError::NoHardwareEncoder(_))
        && err.to_string().contains("E_NO_HARDWARE_ENCODER")
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
async fn record_start_on_running_show_reports_no_encoder_and_stays_idle() {
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

    let err = handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect_err("WU1 has no encoder: every record.start must fail");

    assert!(
        is_no_encoder(&err),
        "expected E_NO_HARDWARE_ENCODER, got: {err}"
    );
    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Idle,
        "refused start must leave the state machine Idle"
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
