//! Prompt 09 WU1 (SPEC §16.14): engine `record.start` / `record.stop`
//! precondition validation + `RecordState` skeleton.
//!
//! Design as built (WU-pipe): `record.start` validates preconditions (RUNNING
//! show, record target, hardware probe — SPEC §16.14), reserves the output
//! path, spawns the dedicated record thread, and flips to Recording. The
//! payload is `{ outputId?: string }` ONLY: show/geometry/rate derive from the
//! loaded package + engine (see `on_record_start` docs). Encoder absence
//! refuses the start with `E_NO_HARDWARE_ENCODER`. `record.stop` requires an
//! active recording and is refused with `E_FORBIDDEN_STATE` while Idle; an
//! empty take fails loudly at finish (`E_RECORD_INPUT`).

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

fn hw_or_skip() -> bool {
    if nbe_engine::encode::is_available() {
        return true;
    }
    eprintln!("SKIP: no hardware H.264 encoder on this machine (SPEC §9.2); recording has no CPU fallback");
    false
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
    if !hw_or_skip() {
        return;
    }
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

    // Start opens: dir configured, probe passes → Ok + Recording + pipeline.
    *state.record_dir.lock().unwrap() = Some(std::env::temp_dir());
    handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({}),
            serde_json::json!({"outputId": "ep01"}),
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
    // The transition exists for the session-less seam (tests arming Recording
    // directly): stop flips Recording -> Idle with nothing to finish.
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
    // The directory check precedes the encoder probe, so no hardware is
    // needed to reach it.
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
