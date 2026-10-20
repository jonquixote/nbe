//! SPEC v0.4.8 row 13: what the engine holds for a declared loop, measured.
//!
//! Preflight reports the residency the engine holds (§12.11.1), so the engine's
//! side is the reference, and it is pinned here on a real decode: the dress's
//! `counter_60.mp4`, 60 distinct 1920x1080 frames, loaded as a 60-frame loop
//! through `show.load`. `v04.rs` asserts preflight's figures for the same
//! package against these.
//!
//! - §12.3's ladder is unbuilt: every loop is held as RGBA8, whatever it
//!   declares.
//! - A loop that fits its budget holds its whole length.
//! - A loop over its budget holds its budget's `maxFramesByBudget` frames: not
//!   a read-ahead window, because this engine does not stream loops (§10.5's
//!   note), and not nothing.
//!
//! No-hardware policy, as Prompt 05: without VideoToolbox these tests FAIL,
//! naming what was missing. They never skip.

use nbe_engine::loop_cache::{CachePolicy, CacheTextureFormat};
use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::path::Path;
use std::sync::Arc;

/// One 1080p frame at RGBA8, in bytes.
const RGBA8_1080P: u64 = 1920 * 1080 * 4;

/// What the engine holds for loop `L` after `show.load`.
struct Held {
    frames: u64,
    bytes: u64,
    format: CacheTextureFormat,
    policy: CachePolicy,
}

/// Load a package whose one loop is `counter_60.mp4` declared as `lp`, and
/// read what the engine decoded and planned for it.
async fn held(lp: serde_json::Value) -> Held {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("media")).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/dress_show/media/counter_60.mp4"),
        root.join("media/loop.mp4"),
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
    std::fs::write(root.join("media/slate.png"), &png).unwrap();
    std::fs::write(
        root.join("manifest.json"),
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
                { "id": "L", "kind": "video", "source": "media/loop.mp4", "format": "h264", "loop": lp }
            ],
            "scenes": [{ "id": "SCN", "elements": [
                { "id": "main", "kind": "videoLoop", "z": 1, "assetId": "L" }
            ]}],
            "rundown": { "id": "R", "items": [
                { "id": "C1", "kind": "sceneRef", "sceneRef": "SCN" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();

    let state = Arc::new(EngineState::new(30));
    DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()))
        .apply(&DirectiveFrame {
            v: PROTOCOL_VERSION.into(),
            kind: DirectiveKind::Directive,
            seq: 1,
            state_version: 1,
            command: "show.load".into(),
            target: serde_json::json!({}),
            payload: serde_json::json!({ "packagePath": root.to_string_lossy() }),
        })
        .await
        .unwrap();
    let library = state.video.lock().unwrap();
    let asset = library.get("L").unwrap_or_else(|| {
        panic!(
            "row 13 needs VideoToolbox to decode the loop; failures: {:?}",
            library.failures
        )
    });
    let frames = asset.frames.len() as u64;
    Held {
        frames,
        bytes: asset
            .frames
            .iter()
            .map(|f| f.rgba.len() as u64)
            .sum::<u64>(),
        format: asset.plan.selected_texture_format,
        policy: asset.plan.cache_policy_selected,
    }
}

#[tokio::test]
async fn a_loop_that_fits_its_budget_holds_its_whole_length_as_rgba8() {
    // §12.3's example at `vramBudgetMib: 512`: 64 RGBA8 1080p frames fit, so
    // the 60-frame loop is resident, all 60 of them, 474.6 MiB.
    let h = held(serde_json::json!({ "periodFrames": 60, "vramBudgetMib": 512 })).await;
    assert_eq!(h.format, CacheTextureFormat::Rgba8);
    assert_eq!(h.policy, CachePolicy::Vram);
    assert_eq!(h.frames, 60, "a loop that fits holds its whole length");
    assert_eq!(h.bytes, 60 * RGBA8_1080P, "60 RGBA8 1080p frames, 474.6 MiB");
}

#[tokio::test]
async fn a_loop_over_its_budget_holds_its_budget_not_a_window_and_not_nothing() {
    // The same loop at §12.4's default 256 MiB: 32 RGBA8 1080p frames fit, so
    // it is planned as streamed, and the engine holds those 32 (253.1 MiB). It
    // does not stream a loop (§10.5's note): the frames it holds are the
    // frames it decoded at load, not a read-ahead window.
    let h = held(serde_json::json!({ "periodFrames": 60 })).await;
    assert_eq!(h.format, CacheTextureFormat::Rgba8);
    assert_eq!(h.policy, CachePolicy::Streaming);
    assert_eq!(h.frames, 32, "a loop over its budget holds its budget's frames");
    assert_eq!(h.bytes, 32 * RGBA8_1080P, "32 RGBA8 1080p frames, 253.1 MiB");
}

#[tokio::test]
async fn a_declared_format_does_not_change_what_the_engine_holds() {
    // A loop declaring `bc7` asks for a quarter of RGBA8. The decode path
    // produces RGBA8 whatever the manifest declares, so the engine holds
    // exactly what it holds for the undeclared loop.
    let h = held(serde_json::json!({
        "periodFrames": 60, "textureFormat": "bc7", "vramBudgetMib": 512
    }))
    .await;
    assert_eq!(h.format, CacheTextureFormat::Rgba8, "the declaration is not held");
    assert_eq!(h.frames, 60);
    assert_eq!(h.bytes, 60 * RGBA8_1080P);
}
