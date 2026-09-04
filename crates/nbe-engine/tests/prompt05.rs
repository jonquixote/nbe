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
                { "id": "main", "kind": "clip", "z": 1, "assetId": "clip" }
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
    assert_eq!(clip_source_index(100, 100, 30), Some(0));
    assert_eq!(clip_source_index(115, 100, 30), Some(15));
    assert_eq!(clip_source_index(130, 100, 30), None, "past the end");
    assert_eq!(clip_source_index(99, 100, 30), None, "before the start");

    // A loop wraps by modulo, with no restart and no special case at the
    // boundary (SPEC §12.1, AC-9).
    for f in 0..100u64 {
        assert_eq!(loop_source_index(f, 0, 10), Some(f % 10));
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

    let mut render = RenderLoop::new(state.clone(), None).await.unwrap();
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
}

#[tokio::test]
async fn an_undecodable_video_leaves_the_view_black_rather_than_crashing() {
    let dir = tempfile::tempdir().unwrap();
    write_video_package(dir.path(), "corrupt.mp4", None);
    let (state, _out) = load(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    let mut render = RenderLoop::new(state.clone(), None).await.unwrap();
    render.render_frame(0, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        [0, 0, 0, 255],
        "a failed asset renders nothing; the engine keeps running"
    );
}
