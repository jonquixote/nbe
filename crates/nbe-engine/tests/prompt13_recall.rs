//! The Prompt 13 re-plan's P1, the recall leg — `snapshot.recall` reaches the
//! engine.
//!
//! The split brain it guards: the control plane forwarded `snapshot.recall`
//! with the snapshot's NAME and the engine routed nothing for it, so a recall
//! moved the control plane's `viewItem` (and its overlays) while the old View
//! stayed on air. Now the control plane sends the recall resolved
//! (`commands/state.ts`) and the engine applies it as a cut through the
//! take's own application (`directive.rs` `on_recall` → `apply_take`).
//!
//! Pixels, the engine's slate bit and its own tick, asserted together through
//! the real render loop and the directive the control plane sends — the
//! `prompt11_slate` shape. The control plane's side, against the real engine
//! binary, is `packages/control-plane/src/recall.e2e.ts`. GPU required; no
//! adapter fails loudly, as in prompt04.

use nbe_engine::audio_control::AudioCommand;
use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, FallbackSource, OutgoingQueue, OverlayPhase};
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, ItemEvent, PROTOCOL_VERSION};
use std::sync::Arc;
use std::time::Duration;

const SLATE: [u8; 4] = [12, 200, 90, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
/// An empty scene clears to black (`render.rs`: "a defined picture").
const BLACK: [u8; 4] = [0, 0, 0, 255];
const ON_TIME: Duration = Duration::from_secs(5);

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

/// The recall exactly as the control plane resolves it (`commands/state.ts`):
/// `target.itemRef` only when the recalled item is not the one on air (`null`
/// for an empty View), the take's resolution of a cut (`resolveTransition`),
/// and the snapshot's overlays wholesale.
fn recall(sv: u64, item: Option<Option<&str>>, overlays: &[&str]) -> DirectiveFrame {
    let target = match item {
        None => serde_json::json!({}),
        Some(r) => serde_json::json!({ "itemRef": r }),
    };
    directive(
        "snapshot.recall",
        sv,
        target,
        serde_json::json!({
            "transition": "cut",
            "audio": { "transition": "follow" },
            "visibleOverlays": overlays,
        }),
    )
}

/// What the control plane forwards for a take (`view.take`, a cut), optionally
/// with a duration, which is what schedules an item's end.
fn take(sv: u64, item: &str, duration_frames: Option<u64>) -> DirectiveFrame {
    let mut payload = serde_json::json!({ "transition": "cut" });
    if let Some(n) = duration_frames {
        payload["durationFrames"] = n.into();
    }
    directive(
        "view.take",
        sv,
        serde_json::json!({ "itemRef": item }),
        payload,
    )
}

/// A1 is red, A2 is blue, and the slate is neither.
fn make_package(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 32, image::Rgba(SLATE)))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": {"width":1920,"height":1080,"frameRate":30,"colorSpace":"rec709"},
                "audio": {"sampleRate":48000,"loudnessTargetLufs":-16,"truePeakDbtp":-1.5},
                "fallbackAssetId": "slate"
            },
            "qualityProfile": "potato",
            "assets": [{ "id": "slate", "kind": "image", "source": "media/slate.png" }],
            "scenes": [
                { "id": "SCN_RED", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ff0000" } }
                ]},
                { "id": "SCN_BLUE", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#0000ff" } }
                ]}
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_RED" },
                { "id": "A2", "kind": "sceneRef", "sceneRef": "SCN_BLUE" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

struct Engine {
    _dir: tempfile::TempDir,
    state: Arc<EngineState>,
    handler: DirectiveHandler,
    outgoing: Arc<OutgoingQueue>,
    render: RenderLoop,
}

/// A loaded, RUNNING engine with A1 taken on air for real (not poked into
/// state), one on-time warm-up frame rendered, and the take's own audio
/// command drained so each test reads only what follows it.
async fn engine(a1_duration_frames: Option<u64>) -> Engine {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path());
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    let render = match RenderLoop::new(state.clone()).await {
        Ok(r) => r,
        Err(e) => panic!("wgpu adapter unavailable; tests must FAIL loudly, not skip: {e}"),
    };
    // Running, so a scheduled end can fire (`schedule_done` drops it for a
    // stopped show).
    state.clock.lock().unwrap().start();
    handler
        .apply(&take(2, "A1", a1_duration_frames))
        .await
        .unwrap();
    let mut e = Engine {
        _dir: dir,
        state,
        handler,
        outgoing,
        render,
    };
    assert_eq!(
        view_at(&mut e.render, F).await,
        RED,
        "precondition: A1 on air"
    );
    e.state.audio_commands.lock().unwrap().clear();
    e.outgoing.drain();
    e
}

fn centre_px(bytes: &[u8]) -> [u8; 4] {
    let idx = (((VIEW_H / 2) * VIEW_W + VIEW_W / 2) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

fn tick_fallback(state: &EngineState) -> bool {
    match nbe_engine::telemetry::build_tick(state) {
        EngineFrame::EngineTelemetry { fields, .. } => fields.fallback_active,
        other => panic!("expected engineTelemetry, got {other:?}"),
    }
}

/// Render `frame` on time and read the View's centre pixel.
async fn view_at(render: &mut RenderLoop, frame: u64) -> [u8; 4] {
    render.injected_view_delay = None;
    assert!(render
        .render_frame(frame, Some(ON_TIME))
        .view_late_by
        .is_none());
    centre_px(&render.readback_view().await)
}

fn ended(frames: &[EngineFrame], item: &str) -> bool {
    frames.iter().any(|f| {
        matches!(f, EngineFrame::ItemEvent { event: ItemEvent::End, item_ref, .. } if item_ref == item)
    })
}

/// The frames these tests render. The clock runs (so a scheduled end can
/// fire), and a take or recall lands at `master + 1`, never mid-frame — so
/// frames are read well past any start the real clock reaches in a test.
const F: u64 = 100;

/// Long enough for a 3-frame item's end (100 ms at 30 fps) to have fired.
const PAST_A_3_FRAME_END: Duration = Duration::from_millis(250);

#[tokio::test]
async fn a_recall_puts_the_recalled_item_on_air_and_the_tick_agrees() {
    // The split brain itself. A1 is on air; the control plane recalls a
    // snapshot whose View is A2. Before the fix the engine routed nothing for
    // `snapshot.recall` and kept A1 on air while the control plane said A2.
    let mut e = engine(None).await;
    e.handler
        .apply(&recall(3, Some(Some("A2")), &[]))
        .await
        .unwrap();
    let px = view_at(&mut e.render, F + 1).await;
    assert_eq!(
        (px, e.state.fallback_active(), tick_fallback(&e.state)),
        (BLUE, false, false),
        "after the recall: (View centre pixel, slate on air, tick fallbackActive) — the recalled state"
    );
}

#[tokio::test]
async fn a_recall_leaves_the_operators_slate_up_release_parity() {
    // Release parity (§10.3): the engine releases the operator's slate exactly
    // where the control plane's clear reaches it. A take clears
    // `fallbackActive`; a recall does not (`state.ts` `recallSnapshot` never
    // touches it). So the recalled item goes on air UNDER the slate, the slate
    // stays, and the tick keeps saying so — as the control plane does.
    let mut e = engine(None).await;
    e.handler
        .apply(&directive(
            "view.fallback",
            3,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(
        view_at(&mut e.render, F + 1).await,
        SLATE,
        "precondition: the slate is up"
    );
    e.handler
        .apply(&recall(4, Some(Some("A2")), &[]))
        .await
        .unwrap();
    let px = view_at(&mut e.render, F + 2).await;
    assert_eq!(
        (
            px,
            e.state.fallback_held_by(FallbackSource::Held),
            tick_fallback(&e.state)
        ),
        (SLATE, true, true),
        "after the recall: (View centre pixel, the operator's slate held, tick fallbackActive)"
    );
    assert_eq!(
        e.state.view_item.lock().unwrap().as_deref(),
        Some("A2"),
        "the recall applied beneath the slate"
    );
    // The slate's own release point still works: a take brings it down.
    e.handler.apply(&take(5, "A1", None)).await.unwrap();
    let px = view_at(&mut e.render, F + 3).await;
    assert_eq!(
        (px, e.state.fallback_active(), tick_fallback(&e.state)),
        (RED, false, false),
        "after the take that follows: the slate is down"
    );
}

#[tokio::test]
async fn a_recall_arms_the_media_and_supersedes_the_outgoing_items_end() {
    // The resolution path is real: the recall goes through the take's own
    // application, not a bare View swap. Two observables only that path
    // produces — the clip bus's source swaps to the recalled item (the media
    // arm), and the playback generation moves on, so the outgoing item's
    // pending end never fires. That second one is the engine's half of "a
    // recall is not a cut for autoFollow": the item the recall took off air
    // cannot complete afterwards and advance the rundown past the snapshot.
    let mut e = engine(Some(3)).await;
    let start = e.state.master_frame().map(|f| f + 1).unwrap_or(0);
    e.handler
        .apply(&recall(3, Some(Some("A2")), &[]))
        .await
        .unwrap();
    let audio = e.state.audio_commands.lock().unwrap().clone();
    assert!(
        matches!(
            audio.as_slice(),
            [AudioCommand::TakeItem { item_ref, mode, crossfade_frames: 0, t0, .. }]
                if item_ref == "A2" && mode == "follow" && *t0 >= start
        ),
        "the recall swaps the clip bus's source to A2 through the take's path, a cut; got {audio:?}"
    );
    tokio::time::sleep(PAST_A_3_FRAME_END).await;
    let out = e.outgoing.drain();
    assert!(
        !ended(&out, "A1"),
        "the outgoing item's pending end is superseded by the recall's generation; got {out:?}"
    );
    assert_eq!(view_at(&mut e.render, F + 1).await, BLUE);
}

#[tokio::test]
async fn a_recall_of_the_item_on_air_leaves_it_playing() {
    // The control plane omits `target.itemRef` when the recalled item is the
    // one already on air (view.cut's "already on view" rule). Nothing moves:
    // no source swap, and the item's own end still arrives — the recall did
    // not restart it.
    let mut e = engine(Some(3)).await;
    e.handler.apply(&recall(3, None, &[])).await.unwrap();
    assert!(
        e.state.audio_commands.lock().unwrap().is_empty(),
        "no source swap for the item already on air"
    );
    tokio::time::sleep(PAST_A_3_FRAME_END).await;
    assert!(
        ended(&e.outgoing.drain(), "A1"),
        "A1 kept playing and completed"
    );
    assert_eq!(view_at(&mut e.render, F + 1).await, RED);
}

#[tokio::test]
async fn a_recall_of_an_empty_view_clears_it_and_silences_the_clip_bus() {
    // A snapshot saved before anything was taken: its View is empty, and the
    // control plane says `itemRef: null`. The View clears, the outgoing item's
    // end is superseded, and the clip bus takes the silent source rather than
    // leaving A1 audible under an empty picture.
    let mut e = engine(Some(3)).await;
    e.handler.apply(&recall(3, Some(None), &[])).await.unwrap();
    assert_eq!(
        view_at(&mut e.render, F + 1).await,
        BLACK,
        "the View is empty"
    );
    assert_eq!(*e.state.view_item.lock().unwrap(), None);
    let audio = e.state.audio_commands.lock().unwrap().clone();
    assert!(
        matches!(
            audio.as_slice(),
            [AudioCommand::TakeItem { asset_id: None, .. }]
        ),
        "the clip bus takes the silent source; got {audio:?}"
    );
    tokio::time::sleep(PAST_A_3_FRAME_END).await;
    assert!(
        !ended(&e.outgoing.drain(), "A1"),
        "A1's pending end is superseded"
    );
}

#[tokio::test]
async fn a_recall_replaces_the_on_air_overlays_wholesale_and_steady() {
    // §25.2 #2: a snapshot is the entire View state, overlay visibility
    // included. The recall's set replaces the on-air set — not a patch — and
    // lands steady, as the control plane's `recallSnapshot` clears the
    // animation phase.
    let e = engine(None).await;
    e.handler
        .apply(&directive(
            "overlay.show",
            3,
            serde_json::json!({ "overlayId": "ov1" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    e.handler.apply(&recall(4, None, &["ov2"])).await.unwrap();
    {
        let overlays = e.state.overlays.lock().unwrap();
        let on_air: Vec<(&str, bool, OverlayPhase)> = overlays
            .iter()
            .map(|(id, o)| (id.as_str(), o.on_air, o.phase))
            .collect();
        assert_eq!(on_air, vec![("ov2", true, OverlayPhase::Steady)]);
    }
    e.handler.apply(&recall(5, None, &[])).await.unwrap();
    assert!(
        e.state.overlays.lock().unwrap().is_empty(),
        "an empty set clears the on-air overlays"
    );
}
