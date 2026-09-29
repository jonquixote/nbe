//! PR #34's fix round — the operator's slate comes down on a take.
//!
//! `view.fallback` holds the slate (`FallbackSource::Held`). The control
//! plane's `take` clears its `fallbackActive`; until this fix the engine kept
//! the slate on air and its tick kept reporting `fallbackActive: true` — a
//! split brain SPEC §13.4.1's take/cut row (take and cut clear the fallback
//! flag) did not describe. One release path per source: `Held` → take/cut
//! (`on_take`), `Watchdog` → the watchdog's recovery (K = 30).
//!
//! Pixels and the engine's own tick, asserted together, through the real
//! render loop and the directives the control plane sends. The control
//! plane's side of the same sequence — its state and its tick against the
//! real engine binary — is `packages/control-plane/src/slate-release.e2e.ts`.
//! GPU required; no adapter fails loudly, as in prompt04.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, FallbackSource, OutgoingQueue};
use nbe_engine::watchdog::WATCHDOG_CLEAR_AFTER_ON_TIME as K;
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION};
use std::sync::Arc;
use std::time::Duration;

const SLATE: [u8; 4] = [12, 200, 90, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const ON_TIME: Duration = Duration::from_secs(5);
const TRIP_BUDGET: Duration = Duration::from_millis(200);
const TRIP_WORK: Duration = Duration::from_millis(700);

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

/// A loaded engine showing A1, with one on-time warm-up frame rendered (a cold
/// adapter's first frame is the expensive one: CI run 36356479347).
async fn engine() -> (
    tempfile::TempDir,
    Arc<EngineState>,
    DirectiveHandler,
    RenderLoop,
) {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path());
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    let mut render = match RenderLoop::new(state.clone()).await {
        Ok(r) => r,
        Err(e) => panic!("wgpu adapter unavailable; tests must FAIL loudly, not skip: {e}"),
    };
    *state.view_item.lock().unwrap() = Some("A1".into());
    assert!(render.render_frame(0, Some(ON_TIME)).view_late_by.is_none());
    (dir, state, handler, render)
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

/// The operator's slate, as the control plane forwards `view.fallback`.
async fn operator_slate(state: &EngineState, handler: &DirectiveHandler, render: &mut RenderLoop) {
    handler
        .apply(&directive(
            "view.fallback",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(
        view_at(render, 1).await,
        SLATE,
        "precondition: the slate is up"
    );
    assert!(state.fallback_held_by(FallbackSource::Held));
    assert!(tick_fallback(state), "precondition: the tick says so");
}

#[tokio::test]
async fn view_fallback_then_take_puts_the_content_back_and_the_tick_agrees() {
    let (_dir, state, handler, mut render) = engine().await;
    assert_eq!(view_at(&mut render, 0).await, RED);
    operator_slate(&state, &handler, &mut render).await;

    // What the control plane forwards for `view.take` (the payload's
    // `transition` defaults to "cut" in its schema).
    handler
        .apply(&directive(
            "view.take",
            3,
            serde_json::json!({ "itemRef": "A2" }),
            serde_json::json!({ "transition": "cut" }),
        ))
        .await
        .unwrap();

    // All three, together: the View shows the content, the engine's slate
    // bit is down, and its tick — what the control plane reads — says so.
    let px = view_at(&mut render, 2).await;
    assert_eq!(
        (px, state.fallback_active(), tick_fallback(&state)),
        (BLUE, false, false),
        "after the take: (View centre pixel, slate on air, tick fallbackActive)"
    );
}

#[tokio::test]
async fn a_cut_releases_the_operators_slate_too() {
    let (_dir, state, handler, mut render) = engine().await;
    operator_slate(&state, &handler, &mut render).await;

    // The engine's own `view.cut` route (§13.4.1's row names both commands;
    // the control plane forwards its cut as `view.take` with a cut
    // transition, which the test above covers).
    handler
        .apply(&directive(
            "view.cut",
            3,
            serde_json::json!({ "itemRef": "A2" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let px = view_at(&mut render, 2).await;
    assert_eq!(
        (px, state.fallback_active(), tick_fallback(&state)),
        (BLUE, false, false),
        "after the cut: (View centre pixel, slate on air, tick fallbackActive)"
    );
}

#[tokio::test]
async fn a_take_does_not_release_the_watchdogs_slate() {
    let (_dir, state, handler, mut render) = engine().await;

    // Trip the watchdog: one frame late by more than two budgets.
    render.injected_view_delay = Some(TRIP_WORK);
    let late = render.render_frame(1, Some(TRIP_BUDGET)).view_late_by;
    assert!(late.is_some_and(|l| l > 2 * TRIP_BUDGET), "{late:?}");
    assert!(state.fallback_held_by(FallbackSource::Watchdog));

    // New content does not cure lateness: the take leaves the watchdog's
    // slate where it is — its release is the recovery's.
    handler
        .apply(&directive(
            "view.take",
            2,
            serde_json::json!({ "itemRef": "A2" }),
            serde_json::json!({ "transition": "cut" }),
        ))
        .await
        .unwrap();
    assert!(
        state.fallback_held_by(FallbackSource::Watchdog),
        "a take must not release the watchdog's slate"
    );
    for f in 0..K - 1 {
        assert_eq!(
            view_at(&mut render, 2 + f).await,
            SLATE,
            "on-time frame {} of K = {K}: the watchdog's slate stays",
            f + 1
        );
    }
    // The K-th on-time frame is drawn with the slate still up: the watchdog
    // hears a frame's lateness after rendering it, and releases on that
    // report. The frame after shows the take's content.
    assert_eq!(view_at(&mut render, 2 + K).await, SLATE);
    assert!(
        !state.fallback_active() && !tick_fallback(&state),
        "released on the K-th on-time frame's report"
    );
    assert_eq!(
        view_at(&mut render, 3 + K).await,
        BLUE,
        "the next frame shows the take's content"
    );
}
