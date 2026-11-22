//! A decoder starved at `show.load` is retried once, as preflight's has been
//! since #160 (determinism item 12, step 2).
//!
//! Without the retry, a transient platform failure while the package loaded
//! (`-12911`, kVTVideoDecoderMalfunctionErr, under AVFoundation's "Cannot
//! Decode") left the clip's asset failed: its item had nothing to draw, and
//! the View read black. Main f093f1e's run 45009155168 read black in both
//! `clip_hold` tests at once, the binary's first failure in 205 runs, with the
//! same tree green on #193's run. That a starved decode was the cause is the
//! hypothesis; this test shows the engine now recovers from one.
//!
//! The starvation is forced through `video::force_transient_decode_failures`,
//! so the test is deterministic. The fixture is `cfr_30.mp4` (30 frames, 30
//! fps). Without VideoToolbox these tests fail, naming what was missing.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_engine::video::force_transient_decode_failures;
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The seam is process-wide: these tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

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

/// A package with one clip asset on item `C1`.
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
                ]}
            ],
            "rundown": { "id": "R", "items": [
                { "id": "C1", "kind": "sceneRef", "sceneRef": "SCN_CLIP", "durationFrames": 30 }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

/// Load the package with the next `starved` video loads forced to fail
/// transiently; the engine state after.
async fn load_starved(starved: u32) -> Arc<EngineState> {
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path());
    let state = Arc::new(EngineState::new(30));
    force_transient_decode_failures(starved);
    let applied = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()))
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await;
    force_transient_decode_failures(0);
    assert!(applied.is_ok(), "show.load applies: {applied:?}");
    state
}

#[tokio::test]
async fn a_decoder_starved_once_at_load_is_retried_and_the_clip_loads() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let state = load_starved(1).await;
    let library = state.video.lock().unwrap();
    assert!(
        library.failures.is_empty(),
        "one starved decode is retried, not a failure: {:?}",
        library.failures
    );
    let clip = library
        .assets
        .get("clip")
        .expect("the clip is resident after the retry");
    assert!(!clip.frames.is_empty(), "the clip decoded frames");
}

#[tokio::test]
async fn the_retry_is_once_a_decoder_starved_twice_fails_the_asset_as_before() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let state = load_starved(2).await;
    let library = state.video.lock().unwrap();
    assert!(
        library.failures.contains_key("clip"),
        "a decoder starved on the retry too is a failure, reported as before: {:?}",
        library.failures
    );
    assert!(!library.assets.contains_key("clip"));
}
