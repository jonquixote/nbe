//! Prompt 04 — headless GPU tests.
//!
//! Every test here exercises a production path and fails when that path is
//! removed. The previous suite did not: with `render()` gutted, 7 of 8 passed.
//! Each section names the path it covers.
//!
//! No-adapter policy: if wgpu cannot find an adapter these tests MUST fail
//! loudly with the adapter error; they never skip.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::gpu::Gpu;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::scene::DecodedImage;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const FRAME_30: Duration = Duration::from_millis(33);

async fn gpu_or_fail() -> Gpu {
    match Gpu::init(None).await {
        Ok(g) => g,
        Err(e) => panic!("wgpu adapter unavailable; tests must FAIL loudly, not skip: {e}"),
    }
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

/// A package whose two scenes are distinguishable by colour, plus a real PNG
/// fallback slate so the fallback test can compare against decoded bytes.
fn make_package(dir: &std::path::Path, slate_rgba: [u8; 4]) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let mut png = Vec::new();
    let img = image::RgbaImage::from_pixel(64, 32, image::Rgba(slate_rgba));
    image::DynamicImage::ImageRgba8(img)
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

async fn loaded_engine(dir: &std::path::Path) -> (Arc<EngineState>, DirectiveHandler, RenderLoop) {
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.to_string_lossy() }),
        ))
        .await
        .unwrap();
    let render = RenderLoop::new(state.clone(), None).await.unwrap();
    (state, handler, render)
}

/// Centre pixel of an RGBA readback of the View target.
fn centre_px(bytes: &[u8]) -> [u8; 4] {
    let idx = (((VIEW_H / 2) * VIEW_W + VIEW_W / 2) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

// ---------------------------------------------------------------------------
// Step 1 — scene resolution and determinism.
// Path: RenderLoop::render_frame → render_bus → the composite pipeline.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn same_frame_twice_produces_identical_pixels() {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path(), [10, 20, 30, 255]);
    let (state, _h, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    render.render_frame(7, None);
    let a = render.readback_view().await;
    render.render_frame(7, None);
    let b = render.readback_view().await;
    assert_eq!(
        a, b,
        "same state + same frame must produce identical pixels"
    );
    // And the pixels are the scene's, not an arbitrary clear.
    assert_eq!(centre_px(&a), [255, 0, 0, 255], "SCN_RED must render red");
}

#[tokio::test]
async fn different_items_produce_different_pixels() {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path(), [10, 20, 30, 255]);
    let (state, _h, mut render) = loaded_engine(dir.path()).await;

    *state.view_item.lock().unwrap() = Some("A1".into());
    render.render_frame(1, None);
    let red = render.readback_view().await;

    *state.view_item.lock().unwrap() = Some("A2".into());
    render.render_frame(2, None);
    let blue = render.readback_view().await;

    assert_eq!(centre_px(&red), [255, 0, 0, 255]);
    assert_eq!(centre_px(&blue), [0, 0, 255, 255]);
    assert_ne!(red, blue, "the View must be a function of show state");
}

// ---------------------------------------------------------------------------
// Step 2 — AC-17 and mix, driven through the directive handler.
// Path: DirectiveHandler::on_take → state.transition → scene_for.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn take_changes_the_view_within_two_frames() {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path(), [10, 20, 30, 255]);
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();

    *state.view_item.lock().unwrap() = Some("A1".into());
    render.render_frame(state.master_frame().unwrap(), None);
    assert_eq!(centre_px(&render.readback_view().await), [255, 0, 0, 255]);

    // The directive arrives at this frame; AC-17 measures from here.
    let arrival = state.master_frame().unwrap();
    handler
        .apply(&directive(
            "view.take",
            2,
            serde_json::json!({ "itemRef": "A2" }),
            serde_json::json!({ "transition": "cut" }),
        ))
        .await
        .unwrap();

    let start = state
        .transition
        .lock()
        .unwrap()
        .as_ref()
        .expect("the take must arm a transition")
        .start_frame;
    assert!(
        start <= arrival + 2,
        "cut must take effect within 2 frames of the directive (AC-17): arrived at {arrival}, scheduled for {start}"
    );

    // Before the scheduled boundary the outgoing scene is still on air; on it,
    // the incoming scene is.
    if start > 0 {
        render.render_frame(start - 1, None);
        assert_eq!(
            centre_px(&render.readback_view().await),
            [255, 0, 0, 255],
            "no transition may take effect before its frame boundary"
        );
    }
    render.render_frame(start, None);
    let changed_at =
        (centre_px(&render.readback_view().await) == [0, 0, 255, 255]).then_some(start);
    assert!(
        changed_at.is_some(),
        "cut must be on air within 2 frames (AC-17)"
    );
}

#[tokio::test]
async fn mix_interpolates_across_its_duration_and_never_mid_frame() {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path(), [10, 20, 30, 255]);
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    handler
        .apply(&directive(
            "view.take",
            2,
            serde_json::json!({ "itemRef": "A2" }),
            serde_json::json!({ "transition": "mix", "durationFrames": 10 }),
        ))
        .await
        .unwrap();
    let start = state
        .transition
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .start_frame;

    // Mid-mix: blended, and identical when re-rendered at the same frame.
    render.render_frame(start + 5, None);
    let mid = centre_px(&render.readback_view().await);
    assert!(
        mid[0] > 0 && mid[2] > 0,
        "mid-mix must blend both scenes; got {mid:?}"
    );
    render.render_frame(start + 5, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        mid,
        "a mix is quantized to the frame, not to wall time"
    );

    // Complete: fully the incoming scene.
    render.render_frame(start + 10, None);
    assert_eq!(centre_px(&render.readback_view().await), [0, 0, 255, 255]);
}

// ---------------------------------------------------------------------------
// Step 3 — deadline accounting, watchdog, preview isolation.
// Path: render_frame's deadline arm → dropped_frames_total + report_frame.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_late_view_frame_counts_and_trips_the_watchdog_via_the_loop() {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path(), [10, 20, 30, 255]);
    let (state, _h, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    // A frame that cannot meet its deadline. The loop measures the lateness
    // itself; nothing hand-pokes the watchdog.
    render.injected_view_delay = Some(Duration::from_millis(60));
    for f in 0..3 {
        let report = render.render_frame(f, Some(FRAME_30));
        assert!(report.view_late_by.is_some(), "frame {f} should be late");
    }

    assert_eq!(
        state.dropped_frames_total.load(Ordering::SeqCst),
        3,
        "every late View frame is a dropped frame (SPEC §10.2)"
    );
    assert!(
        state.fallback_active.load(Ordering::SeqCst),
        "sustained misses must engage the fallback slate (SPEC §10.3)"
    );
    assert_eq!(
        state.degradation_rung(),
        1,
        "preview degrades first under View pressure (SPEC §10.5)"
    );
}

#[tokio::test]
async fn an_on_time_frame_counts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path(), [10, 20, 30, 255]);
    let (state, _h, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    let report = render.render_frame(0, Some(Duration::from_secs(5)));
    assert!(report.view_late_by.is_none());
    assert_eq!(state.dropped_frames_total.load(Ordering::SeqCst), 0);
    assert!(!state.fallback_active.load(Ordering::SeqCst));
    assert_eq!(state.degradation_rung(), 0);
}

#[tokio::test]
async fn a_failing_preview_never_touches_the_view_or_the_drop_count() {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path(), [10, 20, 30, 255]);
    let (state, _h, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    render.fail_preview = true;
    let report = render.render_frame(0, Some(Duration::from_secs(5)));

    assert!(report.preview_failed, "the preview render failed");
    assert_eq!(state.preview_missed.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.dropped_frames_total.load(Ordering::SeqCst),
        0,
        "preview misses are never dropped frames (SPEC §10.2)"
    );
    // The View still rendered its scene.
    assert_eq!(centre_px(&render.readback_view().await), [255, 0, 0, 255]);
}

// ---------------------------------------------------------------------------
// Step 4 — the fallback slate: uploaded at load, shown on engage, available
// while the clock is STOPPED.
// Path: state.fallback_image → sync_package upload → render_bus fallback draw.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fallback_renders_the_decoded_slate_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let slate = [12, 200, 90, 255];
    make_package(dir.path(), slate);
    let (state, _h, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    render.render_frame(0, None);
    assert_eq!(centre_px(&render.readback_view().await), [255, 0, 0, 255]);

    state.fallback_active.store(true, Ordering::SeqCst);
    render.render_frame(1, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        slate,
        "the View must show the decoded slate, not a hardcoded colour"
    );
}

#[tokio::test]
async fn the_slate_renders_while_the_clock_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let slate = [200, 10, 120, 255];
    make_package(dir.path(), slate);
    let (state, _h, mut render) = loaded_engine(dir.path()).await;
    assert_eq!(
        state.clock_state(),
        nbe_engine::clock::ClockState::Stopped,
        "clock has not started"
    );

    state.fallback_active.store(true, Ordering::SeqCst);
    render.render_frame(0, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        slate,
        "an operator needs a picture before show.start"
    );
}

#[tokio::test]
async fn an_undecodable_fallback_becomes_a_generated_slate() {
    // The repo fixture's fallback.png is a text placeholder. The engine must
    // still put a picture on air rather than nothing (SPEC §7.14).
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": "../../tests/fixtures/valid_show_v0.3" }),
        ))
        .await
        .unwrap();

    let img = state.fallback_image();
    assert_eq!((img.width, img.height), (VIEW_W, VIEW_H));
    assert!(img.rgba.iter().any(|b| *b != 0), "slate must not be blank");

    let mut render = RenderLoop::new(state.clone(), None).await.unwrap();
    state.fallback_active.store(true, Ordering::SeqCst);
    render.render_frame(0, None);
    let px = centre_px(&render.readback_view().await);
    assert_ne!(px, [0, 0, 0, 255], "the fixture must still yield a picture");
}

// ---------------------------------------------------------------------------
// Step 5 — the quality profile reaches telemetry through production code.
// Path: RenderLoop::new → state.quality_profile → show.load cap → build_tick.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn quality_profile_is_probed_capped_and_emitted_by_production_code() {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path(), [10, 20, 30, 255]); // manifest requests "potato"
    let state = Arc::new(EngineState::new(30));

    // The renderer writes the probe result; the test does not.
    let _render = RenderLoop::new(state.clone(), None).await.unwrap();
    let probed = state
        .quality_profile
        .lock()
        .unwrap()
        .expect("the probe result must be recorded by RenderLoop::new");

    // show.load applies the manifest's requested cap.
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

    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } =
        nbe_engine::telemetry::build_tick(&state)
    else {
        panic!("expected EngineTelemetry");
    };
    assert_eq!(
        fields.quality_profile,
        Some(nbe_protocol::QualityProfile::Potato),
        "the manifest requested potato; a faster probe ({probed:?}) must not promote it"
    );
}

// ---------------------------------------------------------------------------
// Step 6.8 — the repo fixture renders.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_v0_3_fixture_produces_a_renderable_first_frame() {
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": "../../tests/fixtures/valid_show_v0.3" }),
        ))
        .await
        .unwrap();
    // The fixture's only element is a video clip: unsupported until Prompt 05,
    // so the scene is empty and renders black — a defined picture, not a fault.
    *state.view_item.lock().unwrap() = Some("A1".into());
    let mut render = RenderLoop::new(state.clone(), None).await.unwrap();
    let report = render.render_frame(0, Some(Duration::from_secs(5)));
    assert!(report.view_late_by.is_none());
    assert_eq!(
        centre_px(&render.readback_view().await),
        [0, 0, 0, 255],
        "an empty scene renders black"
    );
    assert_eq!(
        state.dropped_frames_total.load(Ordering::SeqCst),
        0,
        "an unsupported element is a scope boundary, not a dropped frame"
    );
}

// ---------------------------------------------------------------------------
// Units the paths above depend on.
// ---------------------------------------------------------------------------

#[test]
fn generated_slate_is_the_requested_size_and_colour() {
    let img = DecodedImage::generated_slate(4, 2, [1, 2, 3, 255]);
    assert_eq!(img.rgba.len(), 4 * 2 * 4);
    assert_eq!(&img.rgba[0..4], &[1, 2, 3, 255]);
}

#[tokio::test]
async fn adapter_probe_reports_a_profile() {
    let gpu = gpu_or_fail().await;
    assert!(nbe_protocol::QualityProfile::ALL.contains(&gpu.quality));
}
