//! Prompt 07, step 5: the overlay level (SPEC §7.10).
//!
//! Composition order `View = overlay(transition(A, B))`, overlay persistence
//! across takes, master-clock-keyed show/hide animations, and fallback-above-
//! overlays. This prompt's mission is the composition level, the commands, the
//! persistence semantics, and the proof — not the element renderers, which are
//! 07b's scope. Overlay elements resolve through the same `layer_for` walk as
//! scene elements; `ticker`/`clock`/`breakingBanner` glyph rasterization comes
//! later.

use nbe_engine::scene::PackageIndex;

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