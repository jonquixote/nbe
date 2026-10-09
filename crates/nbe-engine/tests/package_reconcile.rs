//! SPEC v0.4.8 row 5 — a resync reconciles the package by load identity.
//!
//! The control plane put `packagePath` on every `show.resync` and the engine
//! ignored it. A package is loaded by `show.load` and nowhere else
//! (`show.start` only starts the clock), so an engine restarted mid-show came
//! back knowing the View, the overlays and the end, but holding no package: it
//! could not render the item the snapshot said was on air, nor anything after
//! it, and the manual recovery was a stop-load-start on air.
//!
//! The snapshot now carries the package and the control plane's load
//! generation, the `stateVersion` of its last `show.load`. The engine knows
//! the generation of the load it applied. Equal: nothing. Different, or none
//! loaded: `show.load`'s own body runs before the rest of the resync. A load
//! that fails is not fatal: the rest applies, the on-air item is reported
//! `missing`, and the generated slate is on air.
//!
//! Pixels, through the real render loop, the prompt13_recall shape. GPU
//! required; no adapter fails loudly. The control plane's side, against the
//! real binary, is `packages/control-plane/src/package-reconcile.e2e.ts`.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{
    command::RESYNC, DirectiveFrame, DirectiveKind, EngineFrame, ItemEvent, PROTOCOL_VERSION,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
/// An empty scene clears to black (`render.rs`: "a defined picture").
const BLACK: [u8; 4] = [0, 0, 0, 255];
/// The generated slate (`state.rs` `fallback_image`): what the slate shows
/// when no packaged slate is resident.
const GENERATED_SLATE: [u8; 4] = [191, 89, 13, 255];
const ON_TIME: Duration = Duration::from_secs(5);
/// A frame well past any take or resync the real clock reaches in a test.
const F: u64 = 100;

/// A package whose item A1 fills the View with `a1` (`#rrggbb`).
fn write_package(dir: &std::path::Path, a1: &str) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        64,
        32,
        image::Rgba([12, 200, 90, 255]),
    ))
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
                { "id": "SCN_A1", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": a1 } }
                ]}
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_A1" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

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

fn show_load(sv: u64, path: &std::path::Path) -> DirectiveFrame {
    directive(
        "show.load",
        sv,
        serde_json::json!({}),
        serde_json::json!({ "packagePath": path.to_string_lossy() }),
    )
}

fn take_a1(sv: u64) -> DirectiveFrame {
    directive(
        "view.take",
        sv,
        serde_json::json!({ "itemRef": "A1" }),
        serde_json::json!({ "transition": "cut" }),
    )
}

/// A `show.resync` as the control plane sends one (`state.ts`
/// `resyncSnapshot`): a running show with A1 on air, and the package pair.
fn resync(sv: u64, package_path: Option<&str>, load_sv: Option<u64>) -> DirectiveFrame {
    directive(
        RESYNC,
        sv,
        serde_json::json!({}),
        serde_json::json!({
            "showState": "RUNNING",
            "packagePath": package_path,
            "packageLoadStateVersion": load_sv,
            "viewItem": "A1",
            "viewItemStartFrame": 0,
            "previewItem": null,
            "visibleOverlays": [],
            "fallbackActive": false,
        }),
    )
}

struct Engine {
    state: Arc<EngineState>,
    handler: DirectiveHandler,
    outgoing: Arc<OutgoingQueue>,
    render: RenderLoop,
}

/// An engine as a restart leaves it: running its render loop, no package.
async fn fresh_engine() -> Engine {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    let render = match RenderLoop::new(state.clone()).await {
        Ok(r) => r,
        Err(e) => panic!("wgpu adapter unavailable; tests must FAIL loudly, not skip: {e}"),
    };
    Engine {
        state,
        handler,
        outgoing,
        render,
    }
}

/// An engine that loaded `path` by `show.load` at stateVersion 1 and took A1.
async fn loaded_engine(path: &std::path::Path) -> Engine {
    let e = fresh_engine().await;
    e.handler.apply(&show_load(1, path)).await.unwrap();
    e.state.clock.lock().unwrap().start();
    e.handler.apply(&take_a1(2)).await.unwrap();
    e
}

fn centre_px(bytes: &[u8]) -> [u8; 4] {
    let idx = (((VIEW_H / 2) * VIEW_W + VIEW_W / 2) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

async fn view(e: &mut Engine) -> [u8; 4] {
    e.render.injected_view_delay = None;
    assert!(e
        .render
        .render_frame(F, Some(ON_TIME))
        .view_late_by
        .is_none());
    centre_px(&e.render.readback_view().await)
}

fn attempts(e: &Engine) -> u64 {
    e.state.package_load_attempts.load(Ordering::SeqCst)
}

fn missing(frames: &[EngineFrame], item: &str) -> usize {
    frames
        .iter()
        .filter(|f| {
            matches!(f, EngineFrame::ItemEvent { event: ItemEvent::Missing, item_ref, .. } if item_ref == item)
        })
        .count()
}

fn tick_fallback(state: &EngineState) -> bool {
    match nbe_engine::telemetry::build_tick(state) {
        EngineFrame::EngineTelemetry { fields, .. } => fields.fallback_active,
        other => panic!("expected engineTelemetry, got {other:?}"),
    }
}

#[tokio::test]
async fn g1_a_restarted_engine_reloads_its_package_and_the_on_air_item_renders_again() {
    // The restart: the engine holds no package, the snapshot names one.
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path(), "#ff0000");
    let mut e = fresh_engine().await;
    assert_eq!(view(&mut e).await, BLACK, "precondition: nothing to render");
    e.handler
        .apply(&resync(5, Some(&dir.path().to_string_lossy()), Some(3)))
        .await
        .unwrap();
    let picture = view(&mut e).await;
    assert_eq!(
        (picture, attempts(&e)),
        (RED, 1),
        "(the View, load attempts): A1 renders again, after one load"
    );
}

#[tokio::test]
async fn g2_a_divergent_package_is_replaced_and_the_previous_show_never_renders() {
    // A stop-load-start while the engine was away: the snapshot names another
    // package. The engine's own is the previous show's.
    let old = tempfile::tempdir().unwrap();
    write_package(old.path(), "#ff0000");
    let new = tempfile::tempdir().unwrap();
    write_package(new.path(), "#00ff00");
    let mut e = loaded_engine(old.path()).await;
    assert_eq!(view(&mut e).await, RED, "precondition: the old show's A1");
    e.handler
        .apply(&resync(9, Some(&new.path().to_string_lossy()), Some(7)))
        .await
        .unwrap();
    let picture = view(&mut e).await;
    assert_eq!(
        (picture, attempts(&e)),
        (GREEN, 2),
        "(the View, load attempts): the new show's A1, not the previous show's"
    );
}

#[tokio::test]
async fn g3_an_equal_identity_loads_nothing() {
    // A blip reconnect: the snapshot names the package the engine loaded, at
    // the generation it loaded it (the `show.load` at stateVersion 1).
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path(), "#ff0000");
    let mut e = loaded_engine(dir.path()).await;
    let boundary = e.state.package_generation.load(Ordering::SeqCst);
    e.handler
        .apply(&resync(3, Some(&dir.path().to_string_lossy()), Some(1)))
        .await
        .unwrap();
    let picture = view(&mut e).await;
    assert_eq!(
        (
            attempts(&e),
            e.state.package_generation.load(Ordering::SeqCst) - boundary,
            picture
        ),
        (1, 0, RED),
        "(load attempts, load boundaries crossed, the View): no load ran"
    );
}

#[tokio::test]
async fn g4_a_same_path_reload_during_the_outage_reloads() {
    // `show.load { mode: "reload" }` while the engine was away: the same path,
    // a new generation, and the content changed on disk. Path equality alone
    // would miss it.
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path(), "#ff0000");
    let mut e = loaded_engine(dir.path()).await;
    assert_eq!(view(&mut e).await, RED, "precondition: A1 as first loaded");
    write_package(dir.path(), "#00ff00");
    e.handler
        .apply(&resync(8, Some(&dir.path().to_string_lossy()), Some(6)))
        .await
        .unwrap();
    let picture = view(&mut e).await;
    assert_eq!(
        (picture, attempts(&e)),
        (GREEN, 2),
        "(the View, load attempts): the reloaded content"
    );
}

#[tokio::test]
async fn g5_a_package_that_does_not_load_is_not_fatal_and_is_reported() {
    // A broken path: the resync still applies, the on-air item is reported
    // missing, the generated slate is on air, and the same identity is never
    // retried by a later resync (no retry loop).
    let mut e = fresh_engine().await;
    let broken = "/nonexistent/nbe-package-reconcile";
    let applied = e.handler.apply(&resync(5, Some(broken), Some(3))).await;
    let first = e.outgoing.drain();
    assert!(
        applied.is_ok(),
        "a load failure must not refuse the resync (the connection would drop): {applied:?}"
    );
    let picture = view(&mut e).await;
    assert_eq!(
        (
            missing(&first, "A1"),
            e.state.view_item.lock().unwrap().clone(),
            e.state.is_running(),
            tick_fallback(&e.state),
            picture,
            attempts(&e),
        ),
        (1, Some("A1".to_string()), true, true, GENERATED_SLATE, 1),
        "(missing A1, the View, running, slate, the picture, attempts): the rest applied, the failure surfaced"
    );
    e.handler
        .apply(&resync(6, Some(broken), Some(3)))
        .await
        .unwrap();
    assert_eq!(
        (attempts(&e), missing(&e.outgoing.drain(), "A1")),
        (1, 1),
        "(attempts, missing): the same identity is not retried, and is still reported"
    );
}

#[tokio::test]
async fn g5b_a_failed_reload_never_leaves_the_previous_show_renderable() {
    // A divergent package whose load fails: the previous show's package is
    // gone, so even after the slate comes down the old A1 does not render.
    let old = tempfile::tempdir().unwrap();
    write_package(old.path(), "#ff0000");
    let mut e = loaded_engine(old.path()).await;
    assert_eq!(view(&mut e).await, RED, "precondition: the old show's A1");
    e.handler
        .apply(&resync(9, Some("/nonexistent/nbe-other-show"), Some(7)))
        .await
        .unwrap();
    let on_slate = view(&mut e).await;
    // The operator's next take releases the slate (release parity).
    e.handler.apply(&take_a1(10)).await.unwrap();
    assert_eq!(
        (on_slate, view(&mut e).await),
        (GENERATED_SLATE, BLACK),
        "(after the failed reload, after the next take): never the previous show's red"
    );
}

#[tokio::test]
async fn a_resync_naming_no_package_leaves_the_engines_package_alone() {
    // Out of row 5's scope: a snapshot with no package is the `show.unload`
    // case, queued as its own item. The engine's package stays.
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path(), "#ff0000");
    let mut e = loaded_engine(dir.path()).await;
    e.handler.apply(&resync(3, None, None)).await.unwrap();
    let picture = view(&mut e).await;
    assert_eq!(
        (attempts(&e), picture),
        (1, RED),
        "(load attempts, the View): untouched"
    );
}

#[tokio::test]
async fn a_resync_whose_package_identity_does_not_read_is_refused_before_anything_applies() {
    // A load generation with no path is incoherent: the control plane's
    // strict parse never sends one, so it is refused, and nothing moves.
    let e = fresh_engine().await;
    let err = e
        .handler
        .apply(&resync(5, None, Some(3)))
        .await
        .expect_err("a generation with no path is refused");
    assert!(
        err.to_string().contains("package identity does not read"),
        "the refusal names the cause: {err}"
    );
    assert_eq!(
        (
            e.state.view_item.lock().unwrap().clone(),
            e.state.is_running(),
            attempts(&e)
        ),
        (None, false, 0),
        "(the View, running, attempts): nothing applied"
    );
}
