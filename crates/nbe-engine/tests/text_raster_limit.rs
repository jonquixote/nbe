//! Review4 step 1 (the user's ruling of 2026-11-16 23:45, work order 2): a
//! text raster cannot kill the text worker.
//!
//! On main 98d4bab a ticker's raster is as wide as its text, twice (the
//! wrap). Past about 170 characters at a 1080p band that is wider than the
//! device's 8192 px texture limit. Nothing capped it, and wgpu's validation
//! error for the oversized texture panics the thread that made it: the text
//! worker, after which no command's text is ever prepared again.
//!
//! What this proves:
//! - a ticker too wide to draw is refused (counted, reason given), and the
//!   next text is prepared as usual;
//! - a `ticker.override` whose text cannot be drawn is refused by the engine
//!   and changes nothing on air;
//! - a texture the device refuses is an error, not a panic;
//! - a job that panics fails alone; the worker rebuilds and goes on.
//!
//! GPU tests: they skip without an adapter, as the other render tests do.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_engine::text::MAX_TEXT_RASTER_PX;
use nbe_engine::text_prep::{make_texture_checked, PrepOutcome, TextBox, TextPrep};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const PACKAGED_FONT: &str = "../../tests/fixtures/overlay_show/media/Amiri-Regular.ttf";
const WAIT: Duration = Duration::from_secs(10);
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// A 1080p ticker band and a lower third's box.
const BAND: [f32; 4] = [0.0, 0.92, 1.0, 0.08];
const LOWER_THIRD: [f32; 4] = [0.05, 0.75, 0.9, 0.12];

fn font_bytes() -> Vec<u8> {
    std::fs::read(PACKAGED_FONT)
        .or_else(|_| std::fs::read("tests/fixtures/overlay_show/media/Amiri-Regular.ttf"))
        .expect("the fixture's packaged font must be readable")
}

/// 400 characters: about twice the ticker's interim limit.
fn long_ticker_text() -> String {
    "BREAKING NEWS ITEM ".repeat(21).chars().take(400).collect()
}

async fn gpu_or_skip() -> Option<nbe_engine::gpu::Gpu> {
    match nbe_engine::gpu::Gpu::init().await {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("SKIP: no GPU adapter on this machine ({e})");
            None
        }
    }
}

fn worker(gpu: &nbe_engine::gpu::Gpu) -> Arc<TextPrep> {
    let prep = TextPrep::spawn(gpu.device.clone(), gpu.queue.clone(), 1920, 1080);
    prep.set_fonts(1, vec![font_bytes()]);
    prep
}

fn boxed(rect: [f32; 4]) -> TextBox {
    TextBox {
        rect,
        inset_px: 0.0,
    }
}

#[test]
fn the_limit_is_the_devices_default_2d_texture_limit() {
    assert_eq!(
        MAX_TEXT_RASTER_PX,
        wgpu::Limits::default().max_texture_dimension_2d
    );
}

#[tokio::test]
async fn a_ticker_too_wide_to_draw_is_refused_and_the_worker_goes_on() {
    let Some(gpu) = gpu_or_skip().await else {
        return;
    };
    let prep = worker(&gpu);
    let long = long_ticker_text();
    let refused = prep.prepare_checked("tick", &long, WHITE, true, boxed(BAND), WAIT);
    println!("400-character ticker: {refused:?}");
    match &refused {
        PrepOutcome::Refused(why) => assert!(
            why.contains("8192"),
            "the refusal names the limit: {why}"
        ),
        other => panic!("a ticker too wide to draw must be refused, got {other:?}"),
    }
    assert_eq!(prep.refused_total.load(Ordering::SeqCst), 1);
    assert_eq!(prep.restarts_total.load(Ordering::SeqCst), 0);

    // The worker is alive: the next text is prepared.
    let next = prep.prepare_checked(
        "lt",
        "FIRST HEADLINE TEXT",
        WHITE,
        false,
        boxed(LOWER_THIRD),
        WAIT,
    );
    assert_eq!(next, PrepOutcome::Ready, "a lower third after the refusal");

    // A refused key is not queued again (the render loop asks every frame).
    let shaped = prep.shaped_total.load(Ordering::SeqCst);
    let (key, spec) = prep.key_and_spec("tick", &long, WHITE, true, BAND, 0.0);
    for _ in 0..5 {
        prep.request(key.clone(), "tick", spec.clone(), true);
    }
    let settle = prep.prepare_checked("lt2", "SECOND", WHITE, false, boxed(LOWER_THIRD), WAIT);
    assert_eq!(settle, PrepOutcome::Ready);
    assert_eq!(
        prep.shaped_total.load(Ordering::SeqCst),
        shaped + 1,
        "only the new lower third was shaped; the refused ticker was not requeued"
    );
    assert_eq!(prep.refused_total.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_texture_the_device_refuses_is_an_error_not_a_panic() {
    let Some(gpu) = gpu_or_skip().await else {
        return;
    };
    let w = MAX_TEXT_RASTER_PX + 100;
    let rgba = vec![0u8; (w * 4) as usize];
    let made = make_texture_checked(&gpu.device, &gpu.queue, w, 1, &rgba, "too-wide");
    println!(
        "a {w}x1 texture: {}",
        made.as_ref().map(|_| "made".to_string()).unwrap_or_else(|e| e.clone())
    );
    assert!(made.is_err(), "the device's limit is refused as an error");
}

#[tokio::test]
async fn a_job_that_panics_fails_alone_and_the_worker_goes_on() {
    let Some(gpu) = gpu_or_skip().await else {
        return;
    };
    let prep = worker(&gpu);
    prep.injected_panic_next_job.store(true, Ordering::SeqCst);
    let failed = prep.prepare_checked("lt", "ONE", WHITE, false, boxed(LOWER_THIRD), WAIT);
    println!("the panicking job: {failed:?}");
    assert!(
        matches!(failed, PrepOutcome::Refused(_)),
        "the panicking job is refused, not left waiting: {failed:?}"
    );
    assert_eq!(prep.restarts_total.load(Ordering::SeqCst), 1);
    let next = prep.prepare_checked("lt", "TWO", WHITE, false, boxed(LOWER_THIRD), WAIT);
    assert_eq!(
        next,
        PrepOutcome::Ready,
        "the worker rebuilt its fonts and prepares the next text"
    );
}

// ---------------------------------------------------------------------------
// The command: ticker.override through the engine's directive handler.

fn directive(command: &str, sv: u64, payload: serde_json::Value) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target: serde_json::json!({}),
        payload,
    }
}

fn write_package(dir: &Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    std::fs::write(dir.join("media/Amiri-Regular.ttf"), font_bytes()).unwrap();
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 10, 10, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();
    let manifest = serde_json::json!({
        "manifestVersion": "0.4",
        "show": { "id": "s", "title": "ticker limit", "fallbackAssetId": "slate",
                  "video": { "frameRate": 30, "resolution": "1920x1080" } },
        "assets": [
            { "id": "amiri", "kind": "font", "source": "media/Amiri-Regular.ttf" },
            { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" }
        ],
        "templates": [
            { "id": "tpl_ticker", "kind": "ticker", "fontAssetIds": ["amiri"], "fields": [{ "name": "text" }] }
        ],
        "scenes": [
            { "id": "SCN_A", "elements": [{ "id": "bg_a", "kind": "graphic", "z": 0,
                "fields": { "color": "#202020" }, "transform": { "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0 } }] }
        ],
        "overlays": [
            { "id": "ol_ticker", "elements": [{ "id": "tick", "kind": "ticker", "z": 1,
                "templateId": "tpl_ticker", "fields": { "text": "THE DECLARED TICKER" },
                "transform": { "x": 0.0, "y": 0.92, "w": 1.0, "h": 0.08 } }] }
        ],
        "rundown": { "items": [ { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_A" } ] }
    });
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn a_ticker_override_that_cannot_be_drawn_is_refused_and_changes_nothing() {
    if gpu_or_skip().await.is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path());
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    // The render loop starts the text worker and hands it the package's faces.
    let mut render = nbe_engine::render::RenderLoop::new(state.clone())
        .await
        .unwrap();
    render.sync_package();

    let long = long_ticker_text();
    let refused = handler
        .apply(&directive(
            "ticker.override",
            2,
            serde_json::json!({ "mode": "replace", "items": [{ "text": long }] }),
        ))
        .await;
    println!("ticker.override of 400 characters: {refused:?}");
    let err = refused.expect_err("a ticker that cannot be drawn is refused");
    assert!(
        err.to_string().contains("nothing changed on air"),
        "the refusal says so: {err}"
    );
    assert!(
        state.graphics.lock().unwrap().ticker_override.is_empty(),
        "nothing was published"
    );

    let ok = handler
        .apply(&directive(
            "ticker.override",
            3,
            serde_json::json!({ "mode": "replace", "items": [{ "text": "A SHORT ITEM" }] }),
        ))
        .await;
    assert!(ok.is_ok(), "the next override is accepted: {ok:?}");
    assert_eq!(
        state.graphics.lock().unwrap().ticker_override,
        vec!["A SHORT ITEM".to_string()],
        "and it is on air"
    );
}
