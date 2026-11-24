//! SPEC §7.16, §16.2 (v0.4.20 row 1): `view.ftb` fades the whole View to
//! black over `durationFrames` of the master clock, as the View's last stage:
//! above the scene, the overlays and the fallback slate. A resync lands at the
//! faded state at once.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::sync::Arc;
use std::time::Instant;

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

/// One red scene and a white fallback slate.
fn make_package(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let mut png = Vec::new();
    let img = image::RgbaImage::from_pixel(64, 32, image::Rgba([255, 255, 255, 255]));
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
                ]}
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_RED" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

async fn on_air(dir: &std::path::Path) -> (Arc<EngineState>, DirectiveHandler, RenderLoop) {
    make_package(dir);
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.to_string_lossy() }),
        ))
        .await
        .unwrap();
    *state.view_item.lock().unwrap() = Some("A1".into());
    // The clock at frame 1000, so `view.ftb` ramps from a real "now".
    state.start_clock_at(1000, Instant::now());
    let render = RenderLoop::new(state.clone())
        .await
        .expect("the fade test needs a wgpu adapter");
    (state, handler, render)
}

async fn centre(render: &mut RenderLoop, frame: u64) -> [u8; 3] {
    render.render_frame(frame, None);
    let px = render.readback_view().await;
    let i = (((VIEW_H / 2) * VIEW_W + VIEW_W / 2) * 4) as usize;
    [px[i], px[i + 1], px[i + 2]]
}

#[tokio::test]
async fn view_ftb_fades_the_view_to_black_over_its_duration() {
    let dir = tempfile::tempdir().unwrap();
    let (state, handler, mut render) = on_air(dir.path()).await;
    let now = state.master_frame().unwrap();
    assert!(
        centre(&mut render, now).await[0] > 240,
        "the red scene is on air"
    );

    handler
        .apply(&directive(
            "view.ftb",
            2,
            serde_json::json!({ "on": true, "durationFrames": 30 }),
        ))
        .await
        .unwrap();
    let start = state.master_frame().unwrap();
    let half = centre(&mut render, start + 15).await;
    assert!(
        (100..=160).contains(&half[0]),
        "halfway through the fade the red is about half: {half:?}"
    );
    assert_eq!(
        centre(&mut render, start + 30).await,
        [0, 0, 0],
        "at its end the View is black"
    );
    assert_eq!(
        centre(&mut render, start + 300).await,
        [0, 0, 0],
        "and stays black"
    );
}

#[tokio::test]
async fn the_fade_covers_the_fallback_slate_and_a_resync_lands_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (state, handler, mut render) = on_air(dir.path()).await;
    handler
        .apply(&directive("view.fallback", 2, serde_json::json!({})))
        .await
        .unwrap();
    let now = state.master_frame().unwrap();
    assert!(
        centre(&mut render, now).await.iter().all(|c| *c > 240),
        "the white slate is up"
    );

    // A resync that says the View is faded: black at once, no ramp, above the
    // slate (the View's last stage).
    handler
        .apply(&directive(
            nbe_protocol::command::RESYNC,
            3,
            serde_json::json!({
                "showState": "RUNNING", "viewItem": "A1", "viewItemStartFrame": 0,
                "previewItem": null, "visibleOverlays": [], "fallbackActive": true,
                "ftbActive": true
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        centre(&mut render, now + 1).await,
        [0, 0, 0],
        "black at once, over the slate"
    );
}
