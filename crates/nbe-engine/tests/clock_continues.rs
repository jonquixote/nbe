//! SPEC §5.9.4, v0.4.16 row 1: a running show's clock continues across a
//! render node restart.
//!
//! The failover drill (2026-11-23) measured it: the restarted engine began
//! its clock at 0 (last old frame 105–134, first new frame 10–23), while the
//! View item's `t0`, which the control plane holds in the show's clock, said
//! the item went on air near frame 0 of the OLD run. The item sat about 170
//! frames before its own start: §5.9.4's "clip jumps back to zero", reopened
//! for restarts. The snapshot now carries `masterFrame`, the frame the show's
//! clock reads now. A render node whose clock is stopped starts it there, and
//! one whose clock already runs keeps its own.
//!
//! On the directive handler alone: no GPU.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{command::RESYNC, DirectiveFrame};
use std::sync::Arc;

fn engine() -> (DirectiveHandler, Arc<EngineState>) {
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    (handler, state)
}

fn resync(sv: u64, master_frame: Option<u64>) -> DirectiveFrame {
    let mut snapshot = serde_json::json!({
        "showState": "RUNNING",
        "viewItem": null,
        "viewItemStartFrame": null,
        "previewItem": null,
        "visibleOverlays": [],
        "fallbackActive": false,
    });
    if let Some(f) = master_frame {
        snapshot["masterFrame"] = f.into();
    }
    DirectiveFrame {
        v: nbe_protocol::PROTOCOL_VERSION.into(),
        kind: nbe_protocol::DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: RESYNC.into(),
        target: serde_json::json!({}),
        payload: snapshot,
    }
}

#[tokio::test]
async fn a_restarted_engine_resumes_the_shows_clock_where_the_snapshot_says() {
    let (handler, state) = engine();
    assert_eq!(
        state.master_frame(),
        None,
        "a fresh engine's clock is stopped"
    );
    handler.apply(&resync(1, Some(5_400))).await.unwrap();
    let f = state
        .master_frame()
        .expect("the resync of a running show starts the clock");
    assert!(
        (5_400..=5_402).contains(&f),
        "the clock resumes at the snapshot's masterFrame (3 minutes in), not at 0: got {f}"
    );
}

#[tokio::test]
async fn a_running_clock_keeps_its_own_frame_on_a_resync() {
    // A reconnect of a healthy engine, or an adoption: its epoch beats the
    // control plane's estimate.
    let (handler, state) = engine();
    state.start_clock();
    handler.apply(&resync(1, Some(9_000))).await.unwrap();
    let f = state.master_frame().unwrap();
    assert!(
        f < 30,
        "a running clock is not moved by the snapshot: got {f}"
    );
}

#[tokio::test]
async fn without_a_master_frame_the_clock_starts_at_zero_as_before() {
    // A control plane older than v0.4.16 sends no `masterFrame`.
    let (handler, state) = engine();
    handler.apply(&resync(1, None)).await.unwrap();
    let f = state.master_frame().unwrap();
    assert!(f < 30, "no masterFrame: the clock starts at 0, got {f}");
}
