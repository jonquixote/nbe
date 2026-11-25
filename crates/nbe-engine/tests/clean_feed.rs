//! SPEC §9.6 (v0.4.22 row 1): the clean feed. A take whose
//! `outputs.record.source` is `"clean"` records the View without its overlays
//! and graphics. The scene stage, the fallback slate and the fade to black
//! stay in it. The View itself, and so the stream, still shows the program.
//!
//! The render-level tests read the two targets. The take tests record a clean
//! take and a program take through the production tick (`run_tick`) and the
//! real record path, then decode the files: the overlay band is in the
//! program file and absent from the clean one.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};

/// One take at a time: the hardware encoder is a shared resource.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

/// A red scene, a green overlay band across y 0.8..0.9, a yellow slate.
/// `record` is the manifest's `outputs.record`.
fn write_package(dir: &Path, record: serde_json::Value) {
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
                "fallbackAssetId": "slate",
                "outputs": { "record": record }
            },
            "assets": [ { "id": "slate", "kind": "image", "source": "media/slate.png" } ],
            "scenes": [
                { "id": "SCN_RED", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ff0000" } } ] }
            ],
            "overlays": [
                { "id": "bug", "elements": [
                    { "id": "band", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#00ff00" },
                      "transform": { "x": 0.0, "y": 0.8, "w": 1.0, "h": 0.1 } } ] }
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_RED" }
            ] },
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

async fn on_air(
    dir: &Path,
    record: serde_json::Value,
) -> Option<(Arc<EngineState>, DirectiveHandler, RenderLoop)> {
    write_package(dir, record);
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
    handler
        .apply(&directive(
            "overlay.show",
            2,
            serde_json::json!({ "overlayId": "bug" }),
        ))
        .await
        .unwrap();
    let Ok(render) = RenderLoop::new(state.clone()).await else {
        eprintln!("SKIP: no wgpu adapter on this machine");
        return None;
    };
    Some((state, handler, render))
}

fn px_at(bytes: &[u8], fx: f32, fy: f32) -> [u8; 4] {
    let x = (fx * VIEW_W as f32) as usize;
    let y = (fy * VIEW_H as f32) as usize;
    let idx = (y * VIEW_W as usize + x) * 4;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const SLATE: [u8; 4] = [200, 200, 0, 255];
const BAND: (f32, f32) = (0.5, 0.85);
const SCENE: (f32, f32) = (0.5, 0.4);

#[tokio::test]
async fn the_clean_target_has_the_scene_and_not_the_overlay_while_the_view_has_both() {
    let dir = tempfile::tempdir().unwrap();
    let Some((_state, _h, mut render)) = on_air(dir.path(), serde_json::json!({})).await else {
        return;
    };
    render.set_clean_feed(true, None).unwrap();
    // Past the overlay's default enter (the clock is stopped, so it starts at
    // frame 1).
    render.render_frame(30, None);
    let view = render.readback_view().await;
    let clean = render.readback_clean().await;
    assert_eq!(
        px_at(&view, BAND.0, BAND.1),
        GREEN,
        "the View carries the overlay"
    );
    assert_eq!(px_at(&view, SCENE.0, SCENE.1), RED);
    assert_eq!(
        px_at(&clean, BAND.0, BAND.1),
        RED,
        "the clean feed has the scene under the band"
    );
    assert_eq!(px_at(&clean, SCENE.0, SCENE.1), RED);
}

#[tokio::test]
async fn the_slate_and_the_fade_are_the_programs_and_stay_on_the_clean_feed() {
    let dir = tempfile::tempdir().unwrap();
    let Some((state, handler, mut render)) = on_air(dir.path(), serde_json::json!({})).await else {
        return;
    };
    render.set_clean_feed(true, None).unwrap();
    state.engage_fallback(nbe_engine::state::FallbackSource::Held);
    render.render_frame(0, None);
    assert_eq!(
        px_at(&render.readback_clean().await, BAND.0, BAND.1),
        SLATE,
        "the slate is on the clean feed"
    );
    state.release_fallback(nbe_engine::state::FallbackSource::Held);

    // A fade that has landed: black on the View and on the clean feed.
    state.start_clock_at(1000, Instant::now());
    handler
        .apply(&directive(
            "view.ftb",
            3,
            serde_json::json!({ "on": true, "durationFrames": 1 }),
        ))
        .await
        .unwrap();
    render.render_frame(1100, None);
    assert_eq!(
        px_at(&render.readback_view().await, SCENE.0, SCENE.1),
        [0, 0, 0, 255]
    );
    assert_eq!(
        px_at(&render.readback_clean().await, SCENE.0, SCENE.1),
        [0, 0, 0, 255],
        "the fade is on the clean feed"
    );
}

#[tokio::test]
async fn off_the_clean_feed_draws_nothing_into_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let Some((_state, _h, mut render)) = on_air(dir.path(), serde_json::json!({})).await else {
        return;
    };
    render.render_frame(0, None);
    // A fresh texture reads all zeros: no pass ran into it.
    assert_eq!(
        px_at(&render.readback_clean().await, SCENE.0, SCENE.1),
        [0, 0, 0, 0]
    );
}

// ---------------------------------------------------------------------------
// Takes through the production tick
// ---------------------------------------------------------------------------

fn hw_or_skip() -> bool {
    if nbe_engine::encode::is_available() && nbe_engine::record::aac::is_available() {
        return true;
    }
    eprintln!("SKIP: no hardware H.264 or AAC encoder on this machine");
    false
}

fn tone_secs(secs: u32) -> Vec<f32> {
    let frames = (48_000 * secs) as usize;
    let mut pcm = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48_000.0).sin();
        pcm.push(v);
        pcm.push(v);
    }
    pcm
}

/// Record `frames` frames through `run_tick`, paced to the house rate, and
/// return the band and scene pixels of a frame from the middle of the file.
async fn take(record: serde_json::Value, cpu: bool) -> Option<([u8; 4], [u8; 4], String)> {
    let dir = tempfile::tempdir().unwrap();
    let (state, handler, mut render) = on_air(dir.path(), record).await?;
    if cpu {
        // A machine with no zero-copy chain: the readback path.
        *state.render_device.lock().unwrap() = None;
    }
    let out = tempfile::tempdir().unwrap();
    *state.record_dir.lock().unwrap() = Some(out.path().to_path_buf());
    handler
        .apply(&directive("show.start", 3, serde_json::json!({})))
        .await
        .unwrap();
    handler
        .apply(&directive(
            "record.start",
            4,
            serde_json::json!({ "outputId": "ep01" }),
        ))
        .await
        .expect("record.start opens the take");
    let (path, tap) = {
        let g = state.record_session.lock().unwrap();
        let s = g.as_ref().expect("session");
        (s.output_path().to_path_buf(), s.tap())
    };
    tap.push(&tone_secs(2));
    let tap_path = format!(
        "{:?}",
        state.record_tap_selection.lock().unwrap().map(|s| s.path)
    );
    let budget = Duration::from_secs_f64(1.0 / 30.0);
    let mut next = Instant::now() + budget;
    for frame in 0..45u64 {
        nbe_engine::tick::run_tick(&mut render, &state, frame, Some(budget)).await;
        let now = Instant::now();
        if next > now {
            tokio::time::sleep(next - now).await;
        }
        next += budget;
    }
    handler
        .apply(&directive("record.stop", 5, serde_json::json!({})))
        .await
        .expect("record.stop finishes the take");
    let mut dec = nbe_decode::DecodeSession::open(&path).expect("the take decodes");
    let mut mid = None;
    let mut n = 0;
    while let Some(f) = dec.next_frame().expect("frame") {
        n += 1;
        if n == 20 {
            mid = Some(f);
        }
    }
    let f = mid.unwrap_or_else(|| panic!("the take has at least 20 frames; it has {n}"));
    assert_eq!((f.width, f.height), (VIEW_W, VIEW_H));
    let at = |fx: f32, fy: f32| {
        let x = (fx * f.width as f32) as usize;
        let y = (fy * f.height as f32) as usize;
        let i = (y * f.width as usize + x) * 4;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
    };
    Some((at(BAND.0, BAND.1), at(SCENE.0, SCENE.1), tap_path))
}

/// H.264 is lossy: a decoded flat fill is near its colour, not on it.
fn is_red(p: [u8; 4]) -> bool {
    p[0] > 200 && p[1] < 60 && p[2] < 60
}
fn is_green(p: [u8; 4]) -> bool {
    p[1] > 200 && p[0] < 60 && p[2] < 60
}

async fn program_and_clean(cpu: bool) {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let tap = if cpu { "cpuReadback" } else { "auto" };
    // The path each take ran on is the point of the pair: the zero-copy test
    // must not pass on readback.
    let want = if cpu {
        "Some(CpuReadback)"
    } else {
        "Some(ZeroCopy)"
    };
    let Some((band, scene, path)) = take(serde_json::json!({ "tapPath": tap }), cpu).await else {
        return;
    };
    if path != want {
        eprintln!("SKIP: the take ran on {path}, not {want} (no zero-copy chain here)");
        return;
    }
    assert!(
        is_green(band),
        "a program take records the overlay ({path}): band {band:?}"
    );
    assert!(is_red(scene), "{scene:?}");
    let Some((band, scene, path)) = take(
        serde_json::json!({ "tapPath": tap, "source": "clean" }),
        cpu,
    )
    .await
    else {
        return;
    };
    assert_eq!(path, want);
    assert!(
        is_red(band),
        "a clean take records the scene under the overlay ({path}): band {band:?}"
    );
    assert!(is_red(scene), "{scene:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clean_take_on_the_zero_copy_path_records_no_overlay_and_a_program_take_does() {
    program_and_clean(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clean_take_by_readback_records_no_overlay_and_a_program_take_does() {
    program_and_clean(true).await;
}
