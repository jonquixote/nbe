//! SPEC §12.8.1 rule 5, as amended on 2026-11-15 (the user's ruling, item 8):
//! a clip that ends with nothing after it holds its last frame on the View
//! for as long as its item stays on the View. A take or a cut releases the
//! hold. An ended clip on the Preview is still not drawn.
//!
//! The rehearsal's black flash (G run 3, 0.77 s at 32.0 s: a 1 s VT3 ran
//! out before the next take) is what this closes.
//!
//! The fixture is `cfr_30.mp4`: 30 frames at 30 fps, one second. Frames 0 to
//! 9 are red, 10 to 19 green and 20 to 29 blue, so frame 29, the last, is
//! blue; a clip that wrapped would show green or red.
//!
//! No-hardware policy, as Prompt 05: without VideoToolbox or a wgpu adapter
//! these tests FAIL, naming what was missing. They never skip.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{RenderLoop, PREVIEW_H, PREVIEW_W, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// The clip's length in frames (cfr_30.mp4: one second at 30 fps).
const CLIP_FRAMES: u64 = 30;
/// How long the clip is left on the View: one second of clip, then three.
const LEFT_FOR: u64 = 4 * 30;

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

/// A package with the one-second clip on item `C1` and a yellow fill on `Y`.
fn write_package(dir: &Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/media/cfr_30.mp4"),
        dir.join("media/clip.mp4"),
    )
    .unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
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
                "video": { "width": 1920, "height": 1080, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" },
                { "id": "clip", "kind": "video", "source": "media/clip.mp4", "format": "h264" }
            ],
            "scenes": [
                { "id": "SCN_CLIP", "elements": [
                    { "id": "main", "kind": "clip", "z": 1, "assetId": "clip" }
                ]},
                { "id": "SCN_YELLOW", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ffff00" } }
                ]}
            ],
            "rundown": { "id": "R", "items": [
                { "id": "C1", "kind": "sceneRef", "sceneRef": "SCN_CLIP", "durationFrames": CLIP_FRAMES },
                { "id": "Y", "kind": "sceneRef", "sceneRef": "SCN_YELLOW" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

async fn loaded(dir: &Path) -> (Arc<EngineState>, RenderLoop) {
    let state = Arc::new(EngineState::new(30));
    DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()))
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.to_string_lossy() }),
        ))
        .await
        .unwrap();
    let render = RenderLoop::new(state.clone())
        .await
        .expect("the clip hold needs a wgpu adapter to read the picture back");
    (state, render)
}

fn centre_of(bytes: &[u8], width: u32, height: u32) -> [u8; 3] {
    let idx = (((height / 2) * width + width / 2) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2]]
}

fn centre(bytes: &[u8]) -> [u8; 3] {
    centre_of(bytes, VIEW_W, VIEW_H)
}

fn blue(px: [u8; 3]) -> bool {
    px[2] > 200 && px[0] < 60 && px[1] < 60
}

fn red(px: [u8; 3]) -> bool {
    px[0] > 200 && px[1] < 60 && px[2] < 60
}

#[tokio::test]
async fn a_one_second_clip_left_three_seconds_holds_its_last_frame_and_never_goes_black() {
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path());
    let (state, mut render) = loaded(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("C1".into());
    state.view_item_start_frame.store(0, Ordering::SeqCst);

    let mut last = None;
    let mut black = Vec::new();
    let mut not_last = Vec::new();
    for f in 0..LEFT_FOR {
        render.render_frame(f, None);
        if f + 1 < CLIP_FRAMES {
            continue;
        }
        let px = centre(&render.readback_view().await);
        if f + 1 == CLIP_FRAMES {
            assert!(blue(px), "frame 29, the clip's last, is blue: {px:?}");
            last = Some(px);
            continue;
        }
        if px == [0, 0, 0] {
            black.push(f);
        } else if Some(px) != last {
            not_last.push((f, px));
        }
    }
    println!(
        "clip hold: a 1 s clip left {} s on the View: {} frames past its end, {} black, {} not its last frame",
        LEFT_FOR / 30,
        LEFT_FOR - CLIP_FRAMES,
        black.len(),
        not_last.len()
    );
    assert!(
        black.is_empty(),
        "past its end the clip holds its last frame; black at frames {black:?}"
    );
    assert!(
        not_last.is_empty(),
        "every frame past the end repeats frame 29 {last:?} exactly (not a wrapped or other frame): {not_last:?}"
    );

    // A take releases the hold: the View's new item draws, not the clip.
    *state.view_item.lock().unwrap() = Some("Y".into());
    state
        .view_item_start_frame
        .store(LEFT_FOR, Ordering::SeqCst);
    render.render_frame(LEFT_FOR, None);
    let px = centre(&render.readback_view().await);
    assert!(
        px[0] > 200 && px[1] > 200 && px[2] < 60,
        "after the take to Y the View is Y's yellow, not the held clip: {px:?}"
    );
}

#[tokio::test]
async fn past_its_end_a_clip_on_the_preview_is_not_drawn() {
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path());
    let (state, mut render) = loaded(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("Y".into());
    *state.preview_item.lock().unwrap() = Some("C1".into());
    state.preview_item_start_frame.store(0, Ordering::SeqCst);

    render.render_frame(5, None);
    let px = centre_of(&render.readback_preview().await, PREVIEW_W, PREVIEW_H);
    assert!(red(px), "frame 5 of the clip on the Preview is red: {px:?}");

    render.render_frame(45, None);
    let px = centre_of(&render.readback_preview().await, PREVIEW_W, PREVIEW_H);
    assert_eq!(
        px,
        [0, 0, 0],
        "the hold is the View's: past its end a clip on the Preview is not drawn"
    );
}
