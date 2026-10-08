//! SPEC v0.4.8 row 3 — a restarted engine still ends the timed item.
//!
//! `on_resync` re-applied the show's state (the View, the overlays, the
//! slate's engage and release) but never scheduled the on-air timed item's
//! end, and the snapshot carried no duration. So an engine restart stranded a
//! timed item on air: no `itemEvent end` ever fired, and autoFollow stopped
//! dead mid-rundown. The snapshot now carries `viewItemEnd` (the item and its
//! REMAINING time, computed by the control plane and clamped at zero), and the
//! resync schedules it through the take's own generation machinery.
//!
//! On the directive handler alone: no GPU, no capability gate, no SKIP. House
//! rate 30: a frame is 33.3 ms. Each "no end yet" read is taken well before
//! the scheduled time, where a late sleep cannot fake a failure; each "the end
//! arrived" read waits at least twice the scheduled time.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{command::RESYNC, DirectiveFrame, EngineFrame, ItemEvent};
use std::sync::Arc;
use std::time::Duration;

/// An engine as it comes up: the show clock is not running until a resync
/// (or a `show.start`) says so.
fn fresh_engine() -> (DirectiveHandler, Arc<EngineState>, Arc<OutgoingQueue>) {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    (handler, state, outgoing)
}

fn directive(
    command: &str,
    sv: u64,
    target: serde_json::Value,
    payload: serde_json::Value,
) -> DirectiveFrame {
    DirectiveFrame {
        v: nbe_protocol::PROTOCOL_VERSION.into(),
        kind: nbe_protocol::DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target,
        payload,
    }
}

/// A `show.resync` as the control plane sends it (`state.ts`
/// `resyncSnapshot`), on a running show, with `extra` merged in.
fn resync(sv: u64, view: Option<&str>, extra: serde_json::Value) -> DirectiveFrame {
    let mut snapshot = serde_json::json!({
        "showState": "RUNNING",
        "viewItem": view,
        "viewItemStartFrame": view.map(|_| 0),
        "previewItem": null,
        "visibleOverlays": [],
        "fallbackActive": false,
    });
    for (k, v) in extra.as_object().expect("an object").iter() {
        snapshot[k] = v.clone();
    }
    directive(RESYNC, sv, serde_json::json!({}), snapshot)
}

/// A timed take, as the control plane sends one (v0.4.8 row 2).
fn take(sv: u64, item: &str, item_duration_frames: Option<u64>) -> DirectiveFrame {
    let mut payload = serde_json::json!({ "transition": "cut" });
    if let Some(n) = item_duration_frames {
        payload["itemDurationFrames"] = n.into();
    }
    directive(
        "view.take",
        sv,
        serde_json::json!({ "itemRef": item }),
        payload,
    )
}

/// The `end` events for `item` among the frames drained so far.
fn ends(frames: &[EngineFrame], item: &str) -> usize {
    frames
        .iter()
        .filter(|f| {
            matches!(f, EngineFrame::ItemEvent { event: ItemEvent::End, item_ref, .. } if item_ref == item)
        })
        .count()
}

async fn after(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

#[tokio::test]
async fn a_restarted_engine_ends_the_timed_item_at_its_remaining_time() {
    // A fresh engine (nothing pending) is told T1 is on air with 9 frames
    // (300 ms) left. Before the fix nothing was ever scheduled.
    let (handler, _state, outgoing) = fresh_engine();
    handler
        .apply(&resync(
            1,
            Some("T1"),
            serde_json::json!({ "viewItemEnd": { "itemRef": "T1", "remainingFrames": 9 } }),
        ))
        .await
        .unwrap();
    after(150).await;
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        0,
        "no end at 150 ms: 300 ms remain"
    );
    after(550).await;
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        1,
        "the end arrives at the remaining 300 ms"
    );
}

#[tokio::test]
async fn an_item_whose_duration_elapsed_during_the_outage_ends_on_receipt() {
    // The clamped zero: the control plane computed no time left, so the end
    // fires on receipt, late by the outage, on purpose.
    let (handler, _state, outgoing) = fresh_engine();
    handler
        .apply(&resync(
            1,
            Some("T1"),
            serde_json::json!({ "viewItemEnd": { "itemRef": "T1", "remainingFrames": 0 } }),
        ))
        .await
        .unwrap();
    after(150).await;
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        1,
        "an overdue item ends on receipt of the resync"
    );
}

#[tokio::test]
async fn a_redundant_resync_on_a_healthy_engine_leaves_exactly_one_end() {
    // A healthy engine already has T1's end pending (12 frames, 400 ms). A
    // resync 100 ms later re-establishes it with 9 frames (300 ms) left: the
    // pending end is superseded, so exactly one end fires, at the right time.
    let (handler, _state, outgoing) = fresh_engine();
    handler
        .apply(&resync(1, None, serde_json::json!({})))
        .await
        .unwrap();
    handler.apply(&take(2, "T1", Some(12))).await.unwrap();
    after(100).await;
    handler
        .apply(&resync(
            3,
            Some("T1"),
            serde_json::json!({ "viewItemEnd": { "itemRef": "T1", "remainingFrames": 9 } }),
        ))
        .await
        .unwrap();
    after(150).await; // 250 ms after the take
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        0,
        "no end at 250 ms after the take"
    );
    after(550).await; // 800 ms after the take
    let first = ends(&outgoing.drain(), "T1");
    after(300).await; // 1100 ms: a second end, if there were one, is due by now
    let later = ends(&outgoing.drain(), "T1");
    assert_eq!(
        (first, later),
        (1, 0),
        "(ends by 800 ms, ends after): one end, never two"
    );
}

#[tokio::test]
async fn an_untimed_item_schedules_nothing_on_resync() {
    // An untimed item carries no end on the snapshot, so a resync schedules
    // nothing ("an untimed item never ends", v0.4.8 row 2), whether the engine
    // is fresh or already showing it.
    let (handler, _state, outgoing) = fresh_engine();
    handler
        .apply(&resync(1, Some("U1"), serde_json::json!({})))
        .await
        .unwrap();
    handler.apply(&take(2, "U2", None)).await.unwrap();
    handler
        .apply(&resync(3, Some("U2"), serde_json::json!({})))
        .await
        .unwrap();
    after(500).await;
    let out = outgoing.drain();
    assert_eq!(
        (ends(&out, "U1"), ends(&out, "U2")),
        (0, 0),
        "untimed items, a fresh resync and a resync after a take: no end"
    );
}

#[tokio::test]
async fn a_resync_that_carries_no_end_leaves_a_pending_end_alone() {
    // A resync without `viewItemEnd` (an untimed item, an item already done,
    // or a control plane from before v0.4.8 row 3) leaves the playback
    // generation alone, so it cannot cancel an end that is legitimately
    // pending on a healthy engine.
    let (handler, _state, outgoing) = fresh_engine();
    handler
        .apply(&resync(1, None, serde_json::json!({})))
        .await
        .unwrap();
    handler.apply(&take(2, "T1", Some(6))).await.unwrap();
    handler
        .apply(&resync(3, Some("T1"), serde_json::json!({})))
        .await
        .unwrap();
    after(600).await; // 6 frames = 200 ms
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        1,
        "the take's own end still fires, once"
    );
}

#[tokio::test]
async fn a_resync_whose_end_does_not_read_or_names_another_item_is_refused_before_anything_applies()
{
    // The take payload's rule: refused, not half-applied. The control plane
    // parses its own end before it sends one, so each of these is its defect.
    for (case, end) in [
        (
            "an unknown key",
            serde_json::json!({ "itemRef": "T1", "remainingFrames": 9, "remainingMs": 300 }),
        ),
        (
            "a negative remaining time",
            serde_json::json!({ "itemRef": "T1", "remainingFrames": -9 }),
        ),
        (
            "another item than the View",
            serde_json::json!({ "itemRef": "T2", "remainingFrames": 9 }),
        ),
    ] {
        let (handler, state, outgoing) = fresh_engine();
        let err = handler
            .apply(&resync(
                1,
                Some("T1"),
                serde_json::json!({ "viewItemEnd": end }),
            ))
            .await
            .expect_err(case);
        assert!(
            err.to_string().contains("viewItemEnd"),
            "{case}: the refusal names the field: {err}"
        );
        assert_eq!(
            (state.view_item.lock().unwrap().clone(), state.is_running()),
            (None, false),
            "{case}: nothing applied, not the View, not the clock"
        );
        after(400).await;
        assert_eq!(
            ends(&outgoing.drain(), "T1"),
            0,
            "{case}: nothing scheduled"
        );
    }
}

#[tokio::test]
async fn a_resync_onto_a_stopped_show_drops_the_end_as_a_running_one_would_not() {
    // `schedule_done`'s own guard: an end that comes due while the show is
    // stopped is dropped, as a healthy engine's pending end would be.
    let (handler, _state, outgoing) = fresh_engine();
    handler
        .apply(&resync(
            1,
            Some("T1"),
            serde_json::json!({
                "showState": "STOPPED",
                "viewItemEnd": { "itemRef": "T1", "remainingFrames": 0 }
            }),
        ))
        .await
        .unwrap();
    after(300).await;
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        0,
        "a stopped show emits no end"
    );
}
