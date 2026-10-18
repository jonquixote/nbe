//! R20 (docs/soak-protocol.md §5): a plain video clip stops drawing after its
//! first 30 frames. The falsification battery, written before any fix.
//!
//! A clip with no declared loop period is decoded only to
//! `CLIP_PRELOAD_FRAMES` (30) at load, the ring's period falls back to that
//! count, `clip_source_index` returns `None` once the source index reaches it,
//! and `draw_for` drops the layer. No test looked at the picture, so nothing
//! saw it. These tests do: each asks the real draw path for a frame past the
//! ring (`RenderLoop::render_frame`, then `readback_view`), the way Prompt 05's
//! pixel tests do.
//!
//! - The weaker form (the drop-detector), on the dress's A1: frame 45 must be a
//!   drawn layer, not the black a dropped layer leaves. A1 is one repeated
//!   picture, so this passes under a hold-last-frame stopgap and under the fix
//!   alike; it catches the drop.
//! - The stronger form (the stopgap-police), on `counter_60.mp4`: rendered
//!   frame 45 must be the source's own frame 45, decoded independently of the
//!   ring and the draw path. It fails under the drop and under the stopgap.
//! - The fixture-integrity guard: every frame of `counter_60.mp4` is distinct,
//!   so a future fixture edit cannot blind the stronger form.
//!
//! No-hardware policy, as Prompt 05: without VideoToolbox or a wgpu adapter
//! these tests FAIL, naming what was missing. They never skip.

use nbe_engine::decode::DecodeSession;
use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Frame 45: past the 30-frame clip preload, inside both clips.
const PAST_THE_RING: u64 = 45;
/// Frame 15: inside the preload, where today's path draws. The control that
/// makes a failure at 45 attributable to R20 and not to the harness.
const INSIDE_THE_RING: u64 = 15;
/// Per-channel tolerance between a rendered and a decoded pixel. Adjacent
/// frames of `counter_60.mp4` differ by at least 37 in red.
const TOLERANCE: i32 = 10;

fn dress_media(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/dress_show/media")
        .join(name)
}

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

/// A house-format package with one plain clip (no loop period) on item `C1`.
fn write_clip_package(dir: &Path, clip: &Path, duration_frames: u64) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    std::fs::copy(clip, dir.join("media/clip.mp4")).unwrap();
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
            "scenes": [{ "id": "SCN", "elements": [
                { "id": "main", "kind": "clip", "z": 1, "assetId": "clip" }
            ]}],
            "rundown": { "id": "R", "items": [
                { "id": "C1", "kind": "sceneRef", "sceneRef": "SCN", "durationFrames": duration_frames }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

/// Load the package and put `C1` on the View from master frame 0.
async fn on_air(dir: &Path) -> RenderLoop {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    DirectiveHandler::new(state.clone(), outgoing)
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.to_string_lossy() }),
        ))
        .await
        .unwrap();
    *state.view_item.lock().unwrap() = Some("C1".into());
    RenderLoop::new(state)
        .await
        .expect("R20 needs a wgpu adapter to read the picture back")
}

fn centre(bytes: &[u8], width: u32, height: u32) -> [u8; 3] {
    let idx = (((height / 2) * width + width / 2) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2]]
}

async fn rendered_centre(render: &mut RenderLoop, frame: u64) -> [u8; 3] {
    render.render_frame(frame, None);
    centre(&render.readback_view().await, VIEW_W, VIEW_H)
}

/// Every frame of a source, decoded by `nbe_decode` on its own: no ring, no
/// preload cap, no draw path.
fn decode_all(path: &Path) -> Vec<nbe_engine::decode::DecodedFrame> {
    let mut session =
        DecodeSession::open(path).expect("R20 needs VideoToolbox to decode the source");
    let mut frames = Vec::new();
    while let Some(f) = session.next_frame().expect("decode") {
        frames.push(f);
    }
    frames
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| (*x as i32 - *y as i32).abs() <= TOLERANCE)
}

#[tokio::test]
async fn r20_weaker_form_a_clip_still_draws_past_its_first_second() {
    // A1 is 150 frames (5 s), one repeated dark blue picture; the View clears
    // to black, so a dropped layer reads black.
    let dir = tempfile::tempdir().unwrap();
    write_clip_package(dir.path(), &dress_media("A1.mp4"), 150);
    let mut render = on_air(dir.path()).await;

    let inside = rendered_centre(&mut render, INSIDE_THE_RING).await;
    assert!(
        inside != [0, 0, 0],
        "control: frame {INSIDE_THE_RING} is inside the preload and must draw A1, got {inside:?}"
    );
    let past = rendered_centre(&mut render, PAST_THE_RING).await;
    assert!(
        past != [0, 0, 0] && close(past, inside),
        "R20: A1 is 150 frames, but at frame {PAST_THE_RING} the clip's layer was not drawn \
         (centre {past:?}, black is the cleared View; A1 drew {inside:?} at frame {INSIDE_THE_RING})"
    );
}

#[tokio::test]
async fn r20_stronger_form_rendered_frame_45_is_the_sources_own_frame_45() {
    let source = dress_media("counter_60.mp4");
    let decoded = decode_all(&source);
    let truth = |n: u64| {
        let f = &decoded[n as usize];
        centre(&f.rgba, f.width, f.height)
    };

    let dir = tempfile::tempdir().unwrap();
    write_clip_package(dir.path(), &source, decoded.len() as u64);
    let mut render = on_air(dir.path()).await;

    let inside = rendered_centre(&mut render, INSIDE_THE_RING).await;
    assert!(
        close(inside, truth(INSIDE_THE_RING)),
        "control: rendered frame {INSIDE_THE_RING} must be the source's frame {INSIDE_THE_RING} \
         {:?}, got {inside:?}",
        truth(INSIDE_THE_RING)
    );
    let past = rendered_centre(&mut render, PAST_THE_RING).await;
    assert!(
        close(past, truth(PAST_THE_RING)),
        "R20: rendered frame {PAST_THE_RING} must be the source's own frame {PAST_THE_RING} {:?}, \
         got {past:?} (black: the layer was dropped; the source's frame 29 {:?}: a held last \
         frame; frame 44 {:?} or 46 {:?}: an off-by-one)",
        truth(PAST_THE_RING),
        truth(29),
        truth(44),
        truth(46)
    );
}

#[test]
fn r20_fixture_integrity_every_counter_frame_is_distinct() {
    // The stripper-test lesson: a guard is only as good as its fixture. If
    // `counter_60.mp4` ever goes static, or shorter than frame 45, the stronger
    // form above goes blind; this fails first.
    let frames = decode_all(&dress_media("counter_60.mp4"));
    assert!(
        frames.len() as u64 > PAST_THE_RING,
        "R20 fixture: counter_60.mp4 must outlive frame {PAST_THE_RING}, decoded {} frames",
        frames.len()
    );
    let mut seen = HashSet::new();
    for f in &frames {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        f.rgba.hash(&mut h);
        assert!(
            seen.insert(h.finish()),
            "R20 fixture: counter_60.mp4 frame {} repeats an earlier frame; every frame must differ",
            f.index
        );
    }
}
