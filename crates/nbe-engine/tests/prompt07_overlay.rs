//! Prompt 07, step 5: the overlay level (SPEC §7.10).
//!
//! Composition order `View = overlay(transition(A, B))`, overlay persistence
//! across takes, master-clock-keyed show/hide animations, and fallback-above-
//! overlays. This prompt's mission is the composition level, the commands, the
//! persistence semantics, and the proof — not the element renderers, which are
//! 07b's scope. Overlay elements resolve through the same `layer_for` walk as
//! scene elements; `ticker`/`clock`/`breakingBanner` glyph rasterization comes
//! later.

use std::path::Path;
use std::sync::Arc;

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::scene::PackageIndex;
use nbe_engine::state::{EngineState, OutgoingQueue, OverlayPhase};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};

fn overlay_manifest() -> serde_json::Value {
    serde_json::json!({
        "manifestVersion": "0.3",
        "network": { "id": "n", "name": "T" },
        "show": {
            "id": "s", "title": "T",
            "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
            "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
            "fallbackAssetId": "slate"
        },
        "assets": [ { "id": "slate", "kind": "image", "source": "media/slate.png" } ],
        "scenes": [ { "id": "SCN", "elements": [] } ],
        "overlays": [
            { "id": "bug", "elements": [
                { "id": "b2", "kind": "graphic", "z": 9, "templateId": "t",
                  "fields": { "color": "#ff0000" } },
                { "id": "b1", "kind": "graphic", "z": 2, "templateId": "t",
                  "fields": { "color": "#00ff00" },
                  "enterAnimation": { "durationFrames": 10 },
                  "exitAnimation": { "durationFrames": 5 } }
            ] },
            { "id": "clock", "elements": [
                { "id": "c1", "kind": "graphic", "z": 1, "templateId": "t",
                  "fields": { "color": "#0000ff" } }
            ] }
        ],
        "rundown": { "id": "R", "items": [ { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" } ] },
        "control": { "bindings": [] }
    })
}

#[test]
fn package_index_holds_overlays_sorted_by_z() {
    let idx = PackageIndex::build(&overlay_manifest(), std::path::Path::new("/nonexistent"));
    let bug = &idx.overlays["bug"];
    let zs: Vec<i64> = bug.iter().map(|e| e.z).collect();
    assert_eq!(zs, vec![2, 9], "overlay elements must be sorted low-z to high-z");
    assert_eq!(idx.overlays.len(), 2, "both overlays are indexed");
}

#[test]
fn overlay_enter_exit_durations_are_read_off_the_elements() {
    let idx = PackageIndex::build(&overlay_manifest(), std::path::Path::new("/nonexistent"));
    // bug: b1 declares enter 10 / exit 5; b2 declares none. The overlay's own
    // animation bound is the max across its elements, so enter = 10.
    assert_eq!(idx.overlay_enter_frames.get("bug"), Some(&10));
    assert_eq!(idx.overlay_exit_frames.get("bug"), Some(&5));
    // clock: no element declares an animation -> bound falls back to 1 frame.
    assert_eq!(idx.overlay_enter_frames.get("clock"), Some(&1));
}

// ---------------------------------------------------------------------------
// Directives (Prompt 07 step 5, §3.2): overlay.show / overlay.hide, and the
// §5.9.4 resync clear rule.
// ---------------------------------------------------------------------------

fn directive(command: &str, sv: u64, target: serde_json::Value, payload: serde_json::Value) -> DirectiveFrame {
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

/// A package whose fallback slate exists on disk, with one overlay carrying a
/// declared 10-frame enter and 5-frame exit.
fn write_overlay_package(dir: &Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
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
            "network": { "id": "n", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [ { "id": "slate", "kind": "image", "source": "media/slate.png" } ],
            "scenes": [ { "id": "SCN", "elements": [] } ],
            "overlays": [
                { "id": "bug", "elements": [
                    { "id": "b1", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ff0000" },
                      "enterAnimation": { "durationFrames": 10 },
                      "exitAnimation": { "durationFrames": 5 } }
                ] }
            ],
            "rundown": { "id": "R", "items": [ { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" } ] },
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

async fn loaded(dir: &Path) -> (Arc<EngineState>, DirectiveHandler) {
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
    (state, handler)
}

#[tokio::test]
async fn overlay_show_keys_off_the_next_frame_boundary_and_reads_enter_frames() {
    let dir = tempfile::tempdir().unwrap();
    write_overlay_package(dir.path());
    let (state, handler) = loaded(dir.path()).await;

    handler
        .apply(&directive(
            "overlay.show",
            2,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    let ovs = state.overlays.lock().unwrap();
    let bug = ovs.get("bug").expect("an on-air overlay must be tracked");
    assert!(bug.on_air, "overlay.show must put the overlay on air");
    assert_eq!(bug.phase, OverlayPhase::Enter);
    // The clock is stopped, so master_frame() is None -> anim_start falls to 1.
    // The point being tested is the +1 boundary discipline, not the absolute
    // number: anim_start is the *next* frame after the command lands.
    assert_eq!(bug.anim_start, 1, "animation keys off the frame after the command");
    assert_eq!(bug.duration_frames, 10, "the declared enter animation is honoured");
}

#[tokio::test]
async fn overlay_show_on_an_on_air_overlay_is_an_idempotent_noop() {
    let dir = tempfile::tempdir().unwrap();
    write_overlay_package(dir.path());
    let (state, handler) = loaded(dir.path()).await;

    handler
        .apply(&directive(
            "overlay.show",
            2,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let before = state.overlays.lock().unwrap().get("bug").copied();

    handler
        .apply(&directive(
            "overlay.show",
            3,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    assert_eq!(
        state.overlays.lock().unwrap().get("bug"),
        before.as_ref(),
        "a repeat show must not reset the animation timeline"
    );
}

#[tokio::test]
async fn resync_with_empty_visible_overlays_clears_the_on_air_set() {
    let dir = tempfile::tempdir().unwrap();
    write_overlay_package(dir.path());
    let (state, handler) = loaded(dir.path()).await;

    handler
        .apply(&directive(
            "overlay.show",
            2,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    handler
        .apply(&directive(
            nbe_protocol::command::RESYNC,
            3,
            serde_json::json!({}),
            serde_json::json!({
                "showState": "RUNNING", "viewItem": null, "previewItem": null,
                "visibleOverlays": []
            }),
        ))
        .await
        .unwrap();

    assert!(
        state.overlays.lock().unwrap().is_empty(),
        "an empty visibleOverlays array MUST clear all on-air overlays (v0.4 §5.9.4)"
    );
}

#[tokio::test]
async fn resync_is_authoritative_when_naming_an_overlay() {
    let dir = tempfile::tempdir().unwrap();
    write_overlay_package(dir.path());
    let (state, handler) = loaded(dir.path()).await;

    handler
        .apply(&directive(
            nbe_protocol::command::RESYNC,
            2,
            serde_json::json!({}),
            serde_json::json!({
                "showState": "RUNNING", "viewItem": null, "previewItem": null,
                "visibleOverlays": ["bug"]
            }),
        ))
        .await
        .unwrap();

    let ovs = state.overlays.lock().unwrap();
    let bug = ovs.get("bug").expect("an overlay named by the snapshot must be on air");
    assert!(bug.on_air, "resync restores the overlay without an animation (steady)");
    assert_eq!(bug.phase, OverlayPhase::Steady);
}