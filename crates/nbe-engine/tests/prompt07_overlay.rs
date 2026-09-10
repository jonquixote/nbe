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
use nbe_engine::scene::{PackageIndex, Transition, TransitionKind};
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
    assert_eq!(
        zs,
        vec![2, 9],
        "overlay elements must be sorted low-z to high-z"
    );
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
    assert_eq!(
        bug.anim_start, 1,
        "animation keys off the frame after the command"
    );
    assert_eq!(
        bug.duration_frames, 10,
        "the declared enter animation is honoured"
    );
}

#[tokio::test]
async fn overlay_animation_override_beats_the_package_bound() {
    let dir = tempfile::tempdir().unwrap();
    write_overlay_package(dir.path());
    let (state, handler) = loaded(dir.path()).await;

    // The package declares enter 10 for "bug"; payload.animation overrides it.
    // Easing, if carried, is ignored (linear alpha ramps — see records entry c).
    handler
        .apply(&directive(
            "overlay.show",
            2,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({ "animation": { "durationFrames": 20, "easing": "easeIn" } }),
        ))
        .await
        .unwrap();

    let bug = state
        .overlays
        .lock()
        .unwrap()
        .get("bug")
        .copied()
        .expect("an on-air overlay must be tracked");
    assert_eq!(
        bug.duration_frames, 20,
        "payload.animation.durationFrames overrides the declared 10"
    );
}

#[tokio::test]
async fn overlay_show_during_exit_revives_the_overlay_not_noops() {
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
            "overlay.hide",
            3,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(
        state.overlays.lock().unwrap().get("bug").map(|o| o.phase),
        Some(OverlayPhase::Exit)
    );

    // A show arriving mid-exit must flip back to Enter, not no-op and let the
    // overlay drop when the exit completes.
    handler
        .apply(&directive(
            "overlay.show",
            4,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let ov = state.overlays.lock().unwrap().get("bug").copied().unwrap();
    assert_eq!(
        ov.phase,
        OverlayPhase::Enter,
        "show during exit must revive the overlay"
    );
    assert_eq!(ov.duration_frames, 10, "re-keyed to the enter animation");
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
    let bug = ovs
        .get("bug")
        .expect("an overlay named by the snapshot must be on air");
    assert!(
        bug.on_air,
        "resync restores the overlay without an animation (steady)"
    );
    assert_eq!(bug.phase, OverlayPhase::Steady);
}
// ---------------------------------------------------------------------------
// Render (Prompt 07 step 5, §3.1): DSK composite + fallback above overlays.
// Mirrors prompt04's headless wgpu pattern. The clock stays STOPPED so frame
// numbers are literal and deterministic; a take is driven by setting the
// transition directly, which is exactly the state `on_take` produces.
// ---------------------------------------------------------------------------

fn stable_overlay(on_air: bool) -> nbe_engine::state::OverlayRuntime {
    nbe_engine::state::OverlayRuntime {
        on_air,
        anim_start: 0,
        duration_frames: 1,
        phase: if on_air {
            nbe_engine::state::OverlayPhase::Steady
        } else {
            nbe_engine::state::OverlayPhase::Exit
        },
    }
}

fn write_render_package(dir: &Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([200, 200, 0, 255]),
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
                "video": { "width": 1920, "height": 1080, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [ { "id": "slate", "kind": "image", "source": "media/slate.png" } ],
            "scenes": [
                { "id": "SCN_RED", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ff0000" } } ] },
                { "id": "SCN_BLUE", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#0000ff" } } ] }
            ],
            "overlays": [
                { "id": "bug", "elements": [
                    { "id": "band", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#00ff00" },
                      "transform": { "x": 0.0, "y": 0.8, "w": 1.0, "h": 0.1 },
                      "enterAnimation": { "durationFrames": 10 },
                      "exitAnimation": { "durationFrames": 5 } }
                ] },
                { "id": "ticker", "elements": [
                    { "id": "strip", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ffff00" },
                      "transform": { "x": 0.0, "y": 0.9, "w": 1.0, "h": 0.1 } }
                ] },
                { "id": "banner", "elements": [
                    { "id": "top", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ff00ff" },
                      "transform": { "x": 0.0, "y": 0.0, "w": 1.0, "h": 0.08 } }
                ] },
                { "id": "clock", "elements": [
                    { "id": "pad", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#00ffff" },
                      "transform": { "x": 0.0, "y": 0.1, "w": 0.1, "h": 0.1 } }
                ] }
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_RED" },
                { "id": "A2", "kind": "sceneRef", "sceneRef": "SCN_BLUE" }
            ] },
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

async fn render_engine(
    dir: &Path,
) -> (
    Arc<EngineState>,
    DirectiveHandler,
    nbe_engine::render::RenderLoop,
) {
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
    let render = nbe_engine::render::RenderLoop::new(state.clone())
        .await
        .unwrap();
    (state, handler, render)
}

fn px_at(bytes: &[u8], fx: f32, fy: f32) -> [u8; 4] {
    let x = (fx * nbe_engine::render::VIEW_W as f32) as usize;
    let y = (fy * nbe_engine::render::VIEW_H as f32) as usize;
    let idx = (y * nbe_engine::render::VIEW_W as usize + x) * 4;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

const GREEN: [u8; 4] = [0, 255, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const SLATE: [u8; 4] = [200, 200, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const CYAN: [u8; 4] = [0, 255, 255, 255];

/// A mix from SCN_RED (A1) to SCN_BLUE (A2), beginning at `start` for
/// `duration` frames.
fn set_mix(state: &Arc<EngineState>, start: u64, duration: u64) {
    *state.transition.lock().unwrap() = Some(Transition {
        from_item: Some("A1".into()),
        from_start_frame: 0,
        to_item: "A2".into(),
        kind: TransitionKind::Mix,
        duration_frames: duration,
        start_frame: start,
    });
    *state.view_item.lock().unwrap() = Some("A2".into());
}

#[tokio::test]
async fn overlay_persists_across_take() {
    let dir = tempfile::tempdir().unwrap();
    write_render_package(dir.path());
    let (state, _handler, mut render) = render_engine(dir.path()).await;

    *state.view_item.lock().unwrap() = Some("A1".into());
    state
        .overlays
        .lock()
        .unwrap()
        .insert("bug".into(), stable_overlay(true));
    // A 15-frame mix between two solid-color scenes; the overlay region is
    // pixel-identical at transition start, midpoint, and end.
    set_mix(&state, 10, 15);

    for frame in [10, 17, 24] {
        render.render_frame(frame, None);
        let px = px_at(&render.readback_view().await, 0.9, 0.85);
        assert_eq!(
            px, GREEN,
            "overlay must be pixel-identical across the mix at frame {frame}"
        );
    }
}

#[tokio::test]
async fn overlay_composites_above_transition() {
    let dir = tempfile::tempdir().unwrap();
    write_render_package(dir.path());
    let (state, _handler, mut render) = render_engine(dir.path()).await;

    *state.view_item.lock().unwrap() = Some("A1".into());
    state
        .overlays
        .lock()
        .unwrap()
        .insert("bug".into(), stable_overlay(true));
    set_mix(&state, 10, 15);

    // Mix midpoint (frame 17): the adjacent non-overlay centre blends red and
    // blue, but the overlay region carries the overlay's exact color, unmixed.
    render.render_frame(17, None);
    let bytes = render.readback_view().await;
    let centre = px_at(&bytes, 0.5, 0.5);
    assert!(
        centre != RED && centre != BLUE,
        "mid-mix centre must be a blend, got {centre:?}"
    );
    assert_eq!(
        px_at(&bytes, 0.9, 0.85),
        GREEN,
        "overlay must composite above the transition blend"
    );
}

#[tokio::test]
async fn show_hide_animation_timing() {
    let dir = tempfile::tempdir().unwrap();
    write_render_package(dir.path());
    let (state, handler, mut render) = render_engine(dir.path()).await;

    *state.view_item.lock().unwrap() = Some("A1".into());
    render.render_frame(0, None);

    // Show at frame F with a 10-frame enter (declared on the band).
    // Clock stopped -> anim_start = 1.
    handler
        .apply(&directive(
            "overlay.show",
            2,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    render.render_frame(0, None);
    assert_eq!(
        px_at(&render.readback_view().await, 0.9, 0.85),
        RED,
        "alpha 0: the scene shows through"
    );
    // Region alpha partial at F+1 ...
    render.render_frame(1, None);
    let first = px_at(&render.readback_view().await, 0.9, 0.85);
    assert!(
        first[1] > 0 && first[1] < 255,
        "first animated frame is partial, got {first:?}"
    );
    // ... full by F+10, steady after.
    render.render_frame(10, None);
    assert_eq!(
        px_at(&render.readback_view().await, 0.9, 0.85),
        GREEN,
        "complete by F + durationFrames"
    );
    render.render_frame(11, None);
    assert_eq!(
        px_at(&render.readback_view().await, 0.9, 0.85),
        GREEN,
        "steady after completion"
    );

    // Hide at G with a 5-frame exit — gone from the on-air set and the frame
    // by completion. Clock stopped, so the hide re-keys at anim_start = 1
    // with duration 5: alpha hits 0 at frame 5 and the runtime drops.
    handler
        .apply(&directive(
            "overlay.hide",
            3,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    render.render_frame(1, None);
    let hiding = px_at(&render.readback_view().await, 0.9, 0.85);
    assert!(
        hiding != GREEN && hiding != RED,
        "exit in flight is a blend, got {hiding:?}"
    );
    render.render_frame(5, None);
    assert_eq!(
        px_at(&render.readback_view().await, 0.9, 0.85),
        RED,
        "gone from the frame by completion"
    );
    assert!(
        state.overlays.lock().unwrap().get("bug").is_none(),
        "gone from the on-air set by completion"
    );
}

#[tokio::test]
async fn animation_immune_to_take() {
    // Enter animation in flight; take at F+3. The region's alpha ramp
    // afterward matches the original master-clock timeline exactly, as if the
    // take had never happened. The take must not touch the overlay timeline
    // (anim_start/duration unchanged), so the overlay is still partial after
    // the take and completes on its original frame.
    //
    // Note: pixel-exact comparison against a no-take reference engine is the
    // wrong proof here — a take changes the scene *under* the semi-transparent
    // overlay, so composited pixels differ even with an identical alpha ramp.
    // The timeline state plus completion frame is the exact-match signal.
    let dir = tempfile::tempdir().unwrap();
    write_render_package(dir.path());
    let (state, handler, mut render) = render_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());
    handler
        .apply(&directive(
            "overlay.show",
            2,
            serde_json::json!({ "overlayId": "bug" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    // anim_start = 1 (clock stopped); take at F+3 = frame 4.
    set_mix(&state, 4, 15);

    // The take left the overlay timeline alone.
    let rt = state
        .overlays
        .lock()
        .unwrap()
        .get("bug")
        .copied()
        .expect("overlay still on air after the take");
    assert_eq!(rt.anim_start, 1, "a take must not re-key anim_start");
    assert_eq!(rt.duration_frames, 10, "a take must not stretch duration");

    render.render_frame(5, None);
    let mid = px_at(&render.readback_view().await, 0.9, 0.85);
    assert!(
        mid[1] > 0 && mid[1] < 255,
        "still animating after the take, got {mid:?}"
    );
    render.render_frame(10, None);
    assert_eq!(
        px_at(&render.readback_view().await, 0.9, 0.85),
        GREEN,
        "overlay completes on its original timeline (frame 10)"
    );
}

#[tokio::test]
async fn fallback_covers_overlays() {
    // All four overlays on air (positions mirror the overlay_show fixture's
    // four identities; solid graphic fills stand in because ticker/clock
    // glyph rasterization is 07b's scope — see the records backlog line).
    // Force fallback; readback the View: no overlay pixels, exact slate color
    // in every overlay region. Recover: the pre-fallback on-air set is back.
    let dir = tempfile::tempdir().unwrap();
    write_render_package(dir.path());
    let (state, handler, mut render) = render_engine(dir.path()).await;

    {
        let mut ovs = state.overlays.lock().unwrap();
        ovs.insert("bug".into(), stable_overlay(true));
        ovs.insert("ticker".into(), stable_overlay(true));
        ovs.insert("banner".into(), stable_overlay(true));
        ovs.insert("clock".into(), stable_overlay(true));
    }

    render.render_frame(1, None);
    let before = render.readback_view().await;
    // Sanity: every overlay region carries its exact color before fallback.
    assert_eq!(px_at(&before, 0.9, 0.85), GREEN, "bug on before fallback");
    assert_eq!(
        px_at(&before, 0.5, 0.95),
        YELLOW,
        "ticker on before fallback"
    );
    assert_eq!(
        px_at(&before, 0.5, 0.04),
        MAGENTA,
        "banner on before fallback"
    );
    assert_eq!(px_at(&before, 0.05, 0.15), CYAN, "clock on before fallback");

    handler
        .apply(&directive(
            "view.fallback",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    render.render_frame(2, None);
    let bytes = render.readback_view().await;
    // No overlay pixels: exact slate color in every overlay region centre.
    for (name, fx, fy) in [
        ("bug", 0.9, 0.85),
        ("ticker", 0.5, 0.95),
        ("banner", 0.5, 0.04),
        ("clock", 0.05, 0.15),
        ("centre", 0.5, 0.5),
    ] {
        assert_eq!(
            px_at(&bytes, fx, fy),
            SLATE,
            "a fallback cut covers the overlay level ({name})"
        );
    }

    // Recovery: the on-air set was never touched by the fallback (runtimes
    // preserved), so clearing the flag brings every overlay straight back,
    // steady. No recovery directive exists; the test clears the flag
    // directly, which is exactly what the flag's owner would do.
    state
        .fallback_active
        .store(false, std::sync::atomic::Ordering::SeqCst);
    render.render_frame(3, None);
    let after = render.readback_view().await;
    assert_eq!(px_at(&after, 0.9, 0.85), GREEN, "bug returns on recovery");
    assert_eq!(
        px_at(&after, 0.5, 0.95),
        YELLOW,
        "ticker returns on recovery"
    );
    assert_eq!(
        px_at(&after, 0.5, 0.04),
        MAGENTA,
        "banner returns on recovery"
    );
    assert_eq!(px_at(&after, 0.05, 0.15), CYAN, "clock returns on recovery");
    assert_eq!(
        state.overlays.lock().unwrap().len(),
        4,
        "the pre-fallback on-air set is back"
    );
}
