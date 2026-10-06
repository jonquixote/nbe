//! SPEC v0.4.8 row 2 — a timed item ends at ITS duration.
//!
//! `schedule_done` is the only source of `itemEvent end`. It used to run only
//! when the take payload carried `durationFrames`, and that field is the
//! TRANSITION's length (`resolveTransition`). So a cut, which carries none,
//! never ended a timed item, and autoFollow never advanced after a cut. A mix
//! ended a timed item at the mix's length, about half a second in, so
//! autoFollow ran away. A recall inherited both through `apply_take`. The take
//! payload now carries the item's duration as its own field,
//! `itemDurationFrames` (`nbe_protocol::TakePayload`), and the end is
//! scheduled from it.
//!
//! An untimed item never ends: nothing in the engine signals end of file, so
//! these tests pin "no end", not an end at EOF. History's warning applies
//! (the midpoint report's AC-4 lesson): `ItemEvent::End` comes only from
//! `schedule_done`, so every test here is falsified in the record.
//!
//! House rate 30: a frame is 33.3 ms. Each "no end yet" read is taken before
//! the scheduled time, where a late sleep cannot fake a failure. Each "the end
//! arrived" read waits at least twice the scheduled time.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, EngineFrame, ItemEvent};
use std::sync::Arc;
use std::time::Duration;

fn make_engine() -> (DirectiveHandler, Arc<EngineState>, Arc<OutgoingQueue>) {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    // Running, so a scheduled end can fire (`schedule_done` drops it for a
    // stopped show).
    state.clock.lock().unwrap().start();
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

/// A take as the control plane sends it (`commands/view.ts` `takePayload`):
/// the resolved transition, plus the item's own duration when it is timed.
fn take(sv: u64, item: &str, payload: serde_json::Value) -> DirectiveFrame {
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
async fn a_cut_to_a_timed_item_ends_at_the_items_duration() {
    // The cut carries no transition length. Before the fix that meant no end,
    // ever: autoFollow never advanced after a cut.
    let (handler, _state, outgoing) = make_engine();
    handler
        .apply(&take(
            1,
            "T1",
            serde_json::json!({ "transition": "cut", "itemDurationFrames": 6 }),
        ))
        .await
        .unwrap();
    after(500).await; // 6 frames = 200 ms
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        1,
        "a cut to a 6-frame item ends it at 200 ms"
    );
}

#[tokio::test]
async fn a_mix_to_a_timed_item_ends_at_the_items_duration_not_the_mixs() {
    // A 3-frame mix (100 ms) to a 15-frame item (500 ms). Before the fix the
    // item ended at the mix's length, about 100 ms in.
    let (handler, state, outgoing) = make_engine();
    handler
        .apply(&take(
            1,
            "T1",
            serde_json::json!({ "transition": "mix", "durationFrames": 3, "itemDurationFrames": 15 }),
        ))
        .await
        .unwrap();
    // The transition reads ITS field: the mix is 3 frames long.
    assert_eq!(
        state
            .transition
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| t.duration_frames),
        Some(3),
        "the mix's length is `durationFrames`"
    );
    after(300).await;
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        0,
        "no end at 300 ms: the item is 500 ms long, whatever the mix's length"
    );
    after(700).await;
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        1,
        "the item ends at its own 500 ms"
    );
}

#[tokio::test]
async fn a_superseded_items_end_is_dropped_and_the_next_items_fires() {
    // A (200 ms) is taken, then B (400 ms) before A's end. A's end is dropped
    // by the playback generation; B's fires.
    let (handler, _state, outgoing) = make_engine();
    handler
        .apply(&take(
            1,
            "A",
            serde_json::json!({ "transition": "cut", "itemDurationFrames": 6 }),
        ))
        .await
        .unwrap();
    handler
        .apply(&take(
            2,
            "B",
            serde_json::json!({ "transition": "cut", "itemDurationFrames": 12 }),
        ))
        .await
        .unwrap();
    after(900).await;
    let out = outgoing.drain();
    assert_eq!(
        (ends(&out, "A"), ends(&out, "B")),
        (0, 1),
        "(A's ends, B's ends): A was superseded, B completed"
    );
}

#[tokio::test]
async fn an_untimed_item_never_ends_and_a_transitions_length_never_ends_it() {
    // An untimed item carries no `itemDurationFrames`. A cut schedules
    // nothing, and a mix's `durationFrames` is the mix's, never the item's.
    // There is no end-of-file signal in the engine.
    let (handler, _state, outgoing) = make_engine();
    handler
        .apply(&take(1, "U1", serde_json::json!({ "transition": "cut" })))
        .await
        .unwrap();
    handler
        .apply(&take(
            2,
            "U2",
            serde_json::json!({ "transition": "mix", "durationFrames": 3 }),
        ))
        .await
        .unwrap();
    after(600).await;
    let out = outgoing.drain();
    assert_eq!(
        (ends(&out, "U1"), ends(&out, "U2")),
        (0, 0),
        "untimed items, a cut and a 3-frame mix: no end"
    );
}

#[tokio::test]
async fn a_timed_item_ends_exactly_once() {
    // No double end for a clip: one scheduled end per take, whatever else the
    // payload carries.
    let (handler, _state, outgoing) = make_engine();
    handler
        .apply(&take(
            1,
            "T1",
            serde_json::json!({ "transition": "mix", "durationFrames": 3, "itemDurationFrames": 3 }),
        ))
        .await
        .unwrap();
    after(700).await; // 3 frames = 100 ms, waited 7x
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        1,
        "exactly one end for one take of a timed item"
    );
}

#[tokio::test]
async fn a_recall_of_a_timed_item_schedules_its_end() {
    // The recall row's new truth (v0.4.8 row 2): the recalled item starts now
    // and ends at its own duration. Before the fix a recall's cut carried no
    // duration and never ended.
    let (handler, _state, outgoing) = make_engine();
    handler
        .apply(&directive(
            "snapshot.recall",
            1,
            serde_json::json!({ "itemRef": "T1" }),
            serde_json::json!({
                "transition": "cut",
                "audio": { "transition": "follow" },
                "itemDurationFrames": 6,
                "visibleOverlays": []
            }),
        ))
        .await
        .unwrap();
    after(500).await;
    assert_eq!(
        ends(&outgoing.drain(), "T1"),
        1,
        "the recalled 6-frame item ends at 200 ms"
    );
}

#[tokio::test]
async fn the_two_durations_are_distinct_fields_on_the_wire() {
    // The collision was the defect's root, so the naming is pinned. The
    // transition's length is `durationFrames` (§16.2's own name, resolved) and
    // the item's is `itemDurationFrames`, in the typed model the engine reads.
    let p = nbe_protocol::TakePayload {
        transition: Some("mix".into()),
        transition_duration_frames: Some(15),
        item_duration_frames: Some(150),
        ..Default::default()
    };
    let wire = serde_json::to_value(&p).unwrap();
    assert_eq!(wire["durationFrames"], 15, "the transition's length");
    assert_eq!(wire["itemDurationFrames"], 150, "the item's duration");
    let back: nbe_protocol::TakePayload =
        serde_json::from_value(serde_json::json!({ "durationFrames": 15 })).unwrap();
    assert_eq!(
        (back.transition_duration_frames, back.item_duration_frames),
        (Some(15), None),
        "`durationFrames` alone is a transition length and never an item duration"
    );
}

#[tokio::test]
async fn a_take_payload_that_does_not_read_is_refused_not_half_applied() {
    // The engine reads the payload through the typed model. A malformed one is
    // a control-plane defect (it parses its own payloads before sending), so
    // it is refused loudly and the View does not move.
    let (handler, state, _outgoing) = make_engine();
    let err = handler
        .apply(&take(
            1,
            "T1",
            serde_json::json!({ "transition": "cut", "itemDurationFrames": "six" }),
        ))
        .await
        .expect_err("a payload whose itemDurationFrames is not a number is refused");
    assert!(
        err.to_string().contains("the take payload does not read"),
        "the refusal names the cause: {err}"
    );
    assert_eq!(
        *state.view_item.lock().unwrap(),
        None,
        "the View did not move"
    );
}

#[tokio::test]
async fn an_unknown_key_in_a_take_payload_is_refused_on_every_path() {
    // PR #37's fix-forward: the typed model is strict, as the control plane's
    // schema is. The two-key pass's M8 renamed the item's key and the engine
    // read the payload cleanly as "untimed", so the item silently never ended.
    // A misspelt key is now a refusal that names it, on the take, the cut (it
    // reaches the same application) and the recall, and the View does not move.
    for command in ["view.take", "view.cut", "snapshot.recall"] {
        let (handler, state, outgoing) = make_engine();
        let err = handler
            .apply(&directive(
                command,
                1,
                serde_json::json!({ "itemRef": "T1" }),
                serde_json::json!({ "transition": "cut", "itemDuration": 6 }),
            ))
            .await
            .expect_err("a payload with an unknown key is refused");
        assert!(
            err.to_string().contains("unknown field `itemDuration`"),
            "{command}: the refusal names the key: {err}"
        );
        assert_eq!(
            *state.view_item.lock().unwrap(),
            None,
            "{command}: the View did not move"
        );
        after(400).await;
        assert_eq!(
            ends(&outgoing.drain(), "T1"),
            0,
            "{command}: nothing was scheduled"
        );
    }
}
