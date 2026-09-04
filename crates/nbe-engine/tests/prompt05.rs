//! Prompt 05 — video decode in the render node.
//!
//! Each test names the production path it covers, and fails when that path is
//! removed (Standards §2a). No-hardware policy: if VideoToolbox or a wgpu
//! adapter is unavailable these tests FAIL, naming what was missing — they
//! never skip.

use nbe_engine::decode::{cadence_holds, cadence_pattern, clip_source_index, loop_source_index};
use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_engine::video::{load_video_asset, SessionPool};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn media(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/media")
        .join(name)
}

fn budget() -> nbe_engine::loop_cache::CacheBudget {
    nbe_engine::loop_cache::CacheBudget {
        per_loop_mib: 1024,
        total_mib: 4096,
        recommended_working_set_mib: None,
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

/// A package with one video-backed scene, built around a generated fixture.
fn write_video_package(dir: &Path, clip: &str, loop_period: Option<u32>) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    std::fs::copy(media(clip), dir.join("media/clip.mp4")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();

    let mut clip_asset = serde_json::json!({
        "id": "clip", "kind": "video", "source": "media/clip.mp4", "format": "h264"
    });
    if let Some(p) = loop_period {
        clip_asset["loop"] = serde_json::json!({ "periodFrames": p, "seamless": true });
    }
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" },
                clip_asset
            ],
            "scenes": [{ "id": "SCN", "elements": [
                // A declared loop period makes this a looping element; without
                // one it is a clip that plays once.
                { "id": "main",
                  "kind": if loop_period.is_some() { "videoLoop" } else { "clip" },
                  "z": 1, "assetId": "clip" }
            ]}],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

async fn load(dir: &Path) -> (Arc<EngineState>, Arc<OutgoingQueue>) {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.to_string_lossy() }),
        ))
        .await
        .unwrap();
    (state, outgoing)
}

fn centre_px(bytes: &[u8]) -> [u8; 4] {
    let idx = (((VIEW_H / 2) * VIEW_W + VIEW_W / 2) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

// ---------------------------------------------------------------------------
// Step 2 — decode. Path: nbe_decode::DecodeSession + video::load_video_asset.
// ---------------------------------------------------------------------------

#[test]
fn decode_is_frame_exact_against_the_reference_clip() {
    let pool = SessionPool::new();
    let asset = load_video_asset("clip", &media("cfr_30.mp4"), &pool, budget(), None, 30)
        .expect("VideoToolbox must decode the reference clip; no skipping");
    assert_eq!(asset.frames.len(), 30, "the reference clip has 30 frames");
    assert_eq!((asset.width, asset.height), (640, 360));

    // The generator paints frames 0-9 red, 10-19 green, 20-29 blue, so a
    // frame identifies itself by its pixels.
    let sample = |i: usize| {
        let f = &asset.frames[i];
        let c = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
        [f.rgba[c], f.rgba[c + 1], f.rgba[c + 2]]
    };
    assert!(
        sample(0)[0] > 200 && sample(0)[1] < 60,
        "frame 0 is red: {:?}",
        sample(0)
    );
    assert!(
        sample(12)[1] > 200 && sample(12)[0] < 60,
        "frame 12 is green: {:?}",
        sample(12)
    );
    assert!(
        sample(25)[2] > 200 && sample(25)[0] < 60,
        "frame 25 is blue: {:?}",
        sample(25)
    );
}

#[test]
fn a_corrupt_asset_fails_loudly_rather_than_decoding_nothing() {
    let pool = SessionPool::new();
    let err = load_video_asset("clip", &media("corrupt.mp4"), &pool, budget(), None, 30)
        .expect_err("a truncated file must not decode");
    assert!(
        err.to_string().contains("no video track") || err.to_string().contains("decode"),
        "unhelpful error: {err}"
    );
}

// ---------------------------------------------------------------------------
// Step 3 — master-clock frame selection (SPEC §12.1).
// ---------------------------------------------------------------------------

#[test]
fn clip_and_loop_indices_are_pure_functions_of_the_master_clock() {
    // A clip plays once from its start frame and then runs out.
    assert_eq!(clip_source_index(100, 100, 30, 30, 30), Some(0));
    assert_eq!(clip_source_index(115, 100, 30, 30, 30), Some(15));
    assert_eq!(
        clip_source_index(130, 100, 30, 30, 30),
        None,
        "past the end"
    );
    assert_eq!(
        clip_source_index(99, 100, 30, 30, 30),
        None,
        "before the start"
    );

    // A loop wraps by modulo, with no restart and no special case at the
    // boundary (SPEC §12.1, AC-9).
    for f in 0..100u64 {
        assert_eq!(loop_source_index(f, 0, 10, 30, 30), Some(f % 10));
    }
}

#[test]
fn ten_consecutive_loop_wraps_are_frame_exact_with_no_restart() {
    let pool = SessionPool::new();
    let asset = load_video_asset("loop", &media("loop_10.mp4"), &pool, budget(), Some(10), 5)
        .expect("the loop fixture must decode");
    assert_eq!(asset.period_frames, 10);
    assert_eq!(
        asset.plan.cache_policy_selected,
        nbe_engine::loop_cache::CachePolicy::Vram,
        "a 10-frame loop is resident, not streamed"
    );

    // AC-9: ten wraps, each frame identical to the first pass.
    let first_pass: Vec<[u8; 4]> = (0..10)
        .map(|i| {
            let f = asset.frame_for(i).expect("resident frame");
            let c = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
            [f.rgba[c], f.rgba[c + 1], f.rgba[c + 2], f.rgba[c + 3]]
        })
        .collect();
    for wrap in 1..10u64 {
        for i in 0..10u64 {
            let f = asset.frame_for(wrap * 10 + i).expect("wrapped frame");
            let c = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
            let px = [f.rgba[c], f.rgba[c + 1], f.rgba[c + 2], f.rgba[c + 3]];
            assert_eq!(
                px, first_pass[i as usize],
                "wrap {wrap} frame {i} differs from the first pass"
            );
        }
    }
    // Distinct frames, so the comparison above is not vacuously true.
    assert!(
        first_pass
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 5,
        "the loop fixture must have distinguishable frames: {first_pass:?}"
    );
}

// ---------------------------------------------------------------------------
// Step 3 — cadence (SPEC §18, AC-4).
// ---------------------------------------------------------------------------

#[test]
fn a_12_fps_source_holds_2_3_2_3_at_30() {
    let pattern = cadence_pattern(12, 30, 30);
    let holds = cadence_holds(&pattern);
    assert_eq!(
        holds,
        vec![3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2],
        "12 fps into 30 fps must alternate 3,2 holds across one second"
    );
    assert_eq!(holds.iter().sum::<u64>(), 30, "one second of house frames");
    assert_eq!(pattern.last(), Some(&11), "12 source frames consumed");
}

#[test]
fn the_cadence_fixture_decodes_at_its_declared_rate() {
    let pool = SessionPool::new();
    let asset = load_video_asset("cad", &media("cadence_12.mp4"), &pool, budget(), None, 30)
        .expect("the cadence fixture must decode");
    assert_eq!(asset.frames.len(), 12);
    assert!(
        (asset.source_frame_rate - 12.0).abs() < 0.01,
        "source rate was {}",
        asset.source_frame_rate
    );
}

// ---------------------------------------------------------------------------
// Step 6 — the decode-session cap (SPEC §24).
// ---------------------------------------------------------------------------

#[test]
fn the_session_cap_refuses_rather_than_letting_the_platform_fail() {
    let pool = SessionPool::with_cap(1);
    let held = pool.acquire().expect("first session");
    let err = load_video_asset("clip", &media("cfr_30.mp4"), &pool, budget(), None, 30)
        .expect_err("the cap must refuse a second session");
    assert!(err.to_string().contains("cap reached"), "got: {err}");
    assert_eq!(pool.refused(), 1);
    drop(held);
    // With the slot returned, the same load succeeds.
    load_video_asset("clip", &media("cfr_30.mp4"), &pool, budget(), None, 30)
        .expect("a freed slot is reusable");
}

#[tokio::test]
async fn telemetry_reports_the_decode_session_peak() {
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "cfr_30.mp4", None);
    let (state, _out) = load(dir.path()).await;

    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } =
        nbe_engine::telemetry::build_tick(&state)
    else {
        panic!("expected EngineTelemetry");
    };
    assert!(
        fields.decode_sessions >= 1,
        "loading a video package must show in decodeSessions, got {}",
        fields.decode_sessions
    );
}

// ---------------------------------------------------------------------------
// The scope boundary Prompt 04 drew, now erased: video renders, and a decode
// failure is a fault rather than a silent skip.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_video_element_renders_its_frame_not_black() {
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "cfr_30.mp4", None);
    let (state, _out) = load(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    let mut render = RenderLoop::new(state.clone()).await.unwrap();
    // Frame 0 of the reference clip is red.
    render.render_frame(0, None);
    let px = centre_px(&render.readback_view().await);
    assert!(
        px[0] > 200 && px[1] < 60,
        "a video element must render its decoded frame, got {px:?}"
    );

    // Frame 12 is green: the picture is a function of the master clock.
    render.render_frame(12, None);
    let px = centre_px(&render.readback_view().await);
    assert!(
        px[1] > 200 && px[0] < 60,
        "the frame shown must follow the clock, got {px:?}"
    );
}

#[tokio::test]
async fn a_12_fps_source_spans_30_house_frames_in_the_rendered_picture() {
    // AC-4, end to end, measured in PIXELS — the only signal that actually
    // depends on the cadence mapping.
    //
    // The first end-to-end attempt asserted that no `itemEvent: end` arrives
    // for a 12 fps take. It could not fail: `ItemEvent::End` is emitted only
    // from `schedule_done`, which is spawned only when the take payload
    // carries `durationFrames`, and that take carried none. Nothing on the
    // wire moves when a clip is exhausted — `viewItem` does not clear and no
    // event fires — so the picture is the observable.
    //
    // `cadence_12.mp4` is 12 frames at 12 fps; frame N is red = N*20. At a
    // 30 fps house rate it must span 30 house frames, so house frame 20 shows
    // source frame 8 (red 160). A 1:1 mapping asks for source frame 20, which
    // does not exist, drops the layer, and renders black.
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "cadence_12.mp4", None);
    let (state, _out) = load(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());
    let mut render = RenderLoop::new(state.clone()).await.unwrap();

    render.render_frame(5, None);
    let early = centre_px(&render.readback_view().await);
    assert!(
        early[0] > 20 && early[0] < 80,
        "house frame 5 must show source frame 2 (red ~40), got {early:?}"
    );

    render.render_frame(20, None);
    let mid = centre_px(&render.readback_view().await);
    assert!(
        mid[0] > 120,
        "house frame 20 must show source frame 8 (red ~160) — a 1:1 mapping \
         would have exhausted this 12-frame clip by house frame 12 and \
         rendered black; got {mid:?}"
    );

    render.render_frame(29, None);
    let last = centre_px(&render.readback_view().await);
    assert!(
        last[0] > 180,
        "house frame 29 must still be on air, showing source frame 11 \
         (red ~220), got {last:?}"
    );
}

#[tokio::test]
async fn a_decode_failure_is_reported_as_an_item_event() {
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "corrupt.mp4", None);
    let (_state, outgoing) = load(dir.path()).await;

    let frames = outgoing.drain();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            nbe_protocol::EngineFrame::ItemEvent {
                event: nbe_protocol::ItemEvent::DecodeError,
                ..
            }
        )),
        "a genuine decode failure is a fault, not a scope boundary: {frames:?}"
    );

    // The frame must not carry the render node's filesystem layout. The full
    // reason stays in the engine's log; the wire gets a stable token.
    let detail = frames
        .iter()
        .find_map(|f| match f {
            nbe_protocol::EngineFrame::ItemEvent {
                event: nbe_protocol::ItemEvent::DecodeError,
                detail,
                ..
            } => detail.clone(),
            _ => None,
        })
        .expect("the fault frame carries a detail");
    assert!(
        !detail.contains('/') && !detail.contains(".mp4"),
        "decodeError detail must not leak a path: {detail:?}"
    );
    assert!(
        ["noVideoTrack", "openFailed", "decodeFailed"].contains(&detail.as_str()),
        "expected a stable token, got {detail:?}"
    );

    // The frame must name a rundown Item — that is what the control plane's
    // §17.3 machine tracks. An asset id is unresolvable to it.
    let item_ref = frames
        .iter()
        .find_map(|f| match f {
            nbe_protocol::EngineFrame::ItemEvent {
                event: nbe_protocol::ItemEvent::DecodeError,
                item_ref,
                ..
            } => Some(item_ref.clone()),
            _ => None,
        })
        .expect("the fault frame names something");
    assert_eq!(
        item_ref, "A1",
        "decodeError must be attributed to the rundown item that shows the \
         asset, not to the asset id"
    );
}

#[tokio::test]
async fn an_undecodable_video_leaves_the_view_black_rather_than_crashing() {
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "corrupt.mp4", None);
    let (state, _out) = load(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    let mut render = RenderLoop::new(state.clone()).await.unwrap();
    render.render_frame(0, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        [0, 0, 0, 255],
        "a failed asset renders nothing; the engine keeps running"
    );
}

// ---------------------------------------------------------------------------
// SPEC §12.1 — `t0`: a clip taken at master frame N starts at ITS frame 0.
// Path: DirectiveHandler::on_take -> view_item_start_frame -> scene_for ->
// draw_for -> clip_source_index.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_take_at_a_nonzero_master_frame_starts_the_clip_at_its_own_frame_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "cfr_30.mp4", None);
    let (state, _out) = load(dir.path()).await;
    let mut render = RenderLoop::new(state.clone()).await.unwrap();

    // The show has been running a while: the take lands at master frame 500,
    // far past the clip's own length.
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    state.clock.lock().unwrap().start();
    state
        .view_item_start_frame
        .store(0, std::sync::atomic::Ordering::SeqCst);
    handler
        .apply(&directive(
            "view.take",
            2,
            serde_json::json!({ "itemRef": "A1" }),
            serde_json::json!({ "transition": "cut" }),
        ))
        .await
        .unwrap();
    // Rewrite the transition's start to a large frame: the take's own arrival
    // frame is wall-clock dependent, and this test is about the arithmetic.
    let start: u64 = 500;
    {
        let mut t = state.transition.lock().unwrap();
        let t = t.as_mut().unwrap();
        t.start_frame = start;
    }
    state
        .view_item_start_frame
        .store(start, std::sync::atomic::Ordering::SeqCst);

    // On its first on-air frame the clip shows ITS frame 0, which is red.
    render.render_frame(start, None);
    let px = centre_px(&render.readback_view().await);
    assert!(
        px[0] > 200 && px[1] < 60,
        "a clip taken at master frame {start} must start at its own frame 0 \
         (red), got {px:?} — t0 is being ignored"
    );

    // Fifteen frames later it shows source frame 15, which is green.
    render.render_frame(start + 15, None);
    let px = centre_px(&render.readback_view().await);
    assert!(
        px[1] > 200 && px[0] < 60,
        "15 frames after the take the clip must show its frame 15 (green), \
         got {px:?}"
    );

    // And past its length it has run out rather than wrapping.
    render.render_frame(start + 40, None);
    let px = centre_px(&render.readback_view().await);
    assert_eq!(
        px,
        [0, 0, 0, 255],
        "a clip past its last frame renders nothing, not a wrapped frame"
    );
}

#[tokio::test]
async fn a_loop_taken_late_wraps_from_its_own_start() {
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "loop_10.mp4", Some(10));
    let (state, _out) = load(dir.path()).await;
    let mut render = RenderLoop::new(state.clone()).await.unwrap();

    let start: u64 = 1000;
    *state.view_item.lock().unwrap() = Some("A1".into());
    state
        .view_item_start_frame
        .store(start, std::sync::atomic::Ordering::SeqCst);

    // The frame at t0 and one full period later must be identical, and the
    // frame one period earlier too: §12.1 holds over the whole timeline.
    render.render_frame(start, None);
    let at_t0 = centre_px(&render.readback_view().await);
    render.render_frame(start + 10, None);
    let one_period_later = centre_px(&render.readback_view().await);
    render.render_frame(start - 10, None);
    let one_period_earlier = centre_px(&render.readback_view().await);

    assert_eq!(at_t0, one_period_later, "a loop repeats every period");
    assert_eq!(
        at_t0, one_period_earlier,
        "pre-roll uses mathematical modulo, not a saturating subtraction"
    );

    // A mid-cycle frame differs, so the equality above is not vacuous.
    render.render_frame(start + 5, None);
    assert_ne!(
        at_t0,
        centre_px(&render.readback_view().await),
        "the loop fixture must have distinguishable frames"
    );
}

// ---------------------------------------------------------------------------
// Prompt 06 Step 0 defects, carried from the P5 review.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resync_naming_no_item_clears_the_old_one() {
    // The state after show.stop: the snapshot says nothing is on air. Reading
    // only the naming case left the previous item rendering.
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "cfr_30.mp4", None);
    let (state, out) = load(dir.path()).await;
    let handler = DirectiveHandler::new(state.clone(), out.clone());

    *state.view_item.lock().unwrap() = Some("A1".into());
    handler
        .apply(&directive(
            nbe_protocol::command::RESYNC,
            9,
            serde_json::json!({}),
            serde_json::json!({
                "showState": "STOPPED",
                "viewItem": serde_json::Value::Null,
                "previewItem": serde_json::Value::Null,
                "itemStates": {},
                "sceneStates": {},
                "visibleOverlays": [],
                "automationHold": false,
                "fallbackActive": false,
                "stateVersion": 9
            }),
        ))
        .await
        .unwrap();

    assert_eq!(
        *state.view_item.lock().unwrap(),
        None,
        "a snapshot saying nothing is on air must clear the View"
    );
    assert!(
        state.transition.lock().unwrap().is_none(),
        "a resync supersedes any in-flight transition"
    );

    // And the View renders black rather than the stale item.
    let mut render = RenderLoop::new(state.clone()).await.unwrap();
    render.render_frame(0, None);
    assert_eq!(centre_px(&render.readback_view().await), [0, 0, 0, 255]);
}

#[tokio::test]
async fn an_armed_preview_clip_survives_a_late_master_clock() {
    // Preview used t0 = 0, so an armed clip previewed as black once the master
    // clock passed the clip's own length.
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "cfr_30.mp4", None);
    let (state, _out) = load(dir.path()).await;
    let mut render = RenderLoop::new(state.clone()).await.unwrap();

    let armed_at: u64 = 500; // far past the clip's 30 frames
    *state.preview_item.lock().unwrap() = Some("A1".into());
    state
        .preview_item_start_frame
        .store(armed_at, std::sync::atomic::Ordering::SeqCst);

    render.render_frame(armed_at, None);
    let bytes = render.readback_preview().await;
    let w = nbe_engine::render::PREVIEW_W;
    let h = nbe_engine::render::PREVIEW_H;
    let idx = (((h / 2) * w + w / 2) * 4) as usize;
    let px = [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]];
    assert!(
        px[0] > 200 && px[1] < 60,
        "an armed clip must preview its first frame regardless of the master \
         clock's position, got {px:?}"
    );
}
