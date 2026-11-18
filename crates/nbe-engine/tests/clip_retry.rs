//! Review4 step 8 (the user's ruling of 2026-11-16 23:45, work order 4),
//! design (c), hold only: a clip that stops decoding mid-play is retried,
//! then given up on and reported, instead of freezing in silence.
//!
//! On main 98d4bab a read-ahead decoder that ended before its clip's end left
//! the clip stalled for good (`stalled_at`), the View holding the last frame
//! with nothing told to anyone, and the item's end timer later sent an `end`
//! for it as if it had played. Now: retries after 100, 300 and 900 ms; then
//! the item on the View is reported `decodeError` with a `held` detail, its
//! audio fades out over 500 ms, and its end timer sends no `end`.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, ItemEvent, PROTOCOL_VERSION};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn directive(
    command: &str,
    sv: u64,
    payload: serde_json::Value,
    target: serde_json::Value,
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

fn dress_media(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/dress_show/media")
        .join(name)
}

/// One plain clip (no loop) on item `C1`, as R20's tests build it.
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

/// The dead clip's end timer: an item marked failed on the View sends no
/// `end` when its duration runs out. (The control: `prompt03`'s
/// `item_end_emitted_after_timed_duration` is the same take without the
/// mark, and it sends one.)
#[tokio::test]
async fn a_failed_items_end_timer_sends_no_end() {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    state.clock.lock().unwrap().start();
    handler
        .apply(&directive(
            "view.take",
            7,
            serde_json::json!({ "transition": "cut", "itemDurationFrames": 3 }),
            serde_json::json!({ "itemRef": "A1" }),
        ))
        .await
        .unwrap();
    // The render loop gave up on A1's clip (what `clip_given_up` does).
    state.failed_items.lock().unwrap().insert("A1".into());
    // 3 frames at 30 fps = 100 ms. Wait past it.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let ends = outgoing
        .drain()
        .into_iter()
        .filter(|f| matches!(f, EngineFrame::ItemEvent { event: ItemEvent::End, item_ref, .. } if item_ref == "A1"))
        .count();
    assert_eq!(ends, 0, "no end for an item whose clip failed on the View");

    // A take of the item again starts it anew: its end comes.
    handler
        .apply(&directive(
            "view.take",
            8,
            serde_json::json!({ "transition": "cut", "itemDurationFrames": 3 }),
            serde_json::json!({ "itemRef": "A1" }),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        outgoing
            .drain()
            .iter()
            .any(|f| matches!(f, EngineFrame::ItemEvent { event: ItemEvent::End, item_ref, .. } if item_ref == "A1")),
        "a new take of A1 clears the mark, and its end is sent"
    );
}

/// A clip whose file is cut short while it plays: the decoder stops early,
/// is retried three times, then the item on the View is reported, held.
#[tokio::test]
async fn a_clip_cut_short_mid_play_is_retried_then_reported_held() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("media/clip.mp4");
    write_clip_package(dir.path(), &dress_media("counter_150.mp4"), 150);
    let state = Arc::new(EngineState::new(30));
    DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()))
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    *state.view_item.lock().unwrap() = Some("C1".into());
    let mut render = match RenderLoop::new(state.clone()).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("SKIP: no wgpu adapter ({e})");
            return;
        }
    };
    // Playing: the window fills from frame 0.
    assert!(
        render.settle_read_ahead(0, Duration::from_secs(20)),
        "precondition: the clip primes and plays"
    );
    // The file cut to a quarter, in place (the same inode the decoder holds).
    let len = std::fs::metadata(&file).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(len / 4)
        .unwrap();

    // Play on, a frame every 33 ms, until the clip is given up on or 8 s pass.
    let started = Instant::now();
    let mut frame = 0u64;
    let mut reported = None;
    while started.elapsed() < Duration::from_secs(8) {
        render.settle_read_ahead(frame, Duration::ZERO);
        if let Some(ev) = state.item_events.lock().unwrap().first().cloned() {
            reported = Some(ev);
            break;
        }
        frame = (frame + 1).min(149);
        std::thread::sleep(Duration::from_millis(33));
    }
    let retries = state.clip_retries_total.load(Ordering::SeqCst);
    let failures = state.clip_failures_total.load(Ordering::SeqCst);
    println!(
        "clip cut short: reported after {:.2} s at master frame {frame}: {reported:?}; retries {retries}, failures {failures}",
        started.elapsed().as_secs_f64()
    );
    let (item, event, detail) = reported.expect("the clip on the View is reported within 8 s");
    assert_eq!(item, "C1");
    assert_eq!(event, ItemEvent::DecodeError);
    let detail = detail.unwrap_or_default();
    // The prefix the control plane reads (nbe_protocol::HELD_DETAIL_PREFIX),
    // then the code.
    assert!(
        detail.starts_with(nbe_protocol::HELD_DETAIL_PREFIX),
        "a held decode failure begins with the shared prefix: {detail}"
    );
    assert!(
        detail.starts_with("held: E_CLIP_DECODE"),
        "a held decode failure: {detail}"
    );
    assert_eq!(retries, 3, "three retries before giving up");
    assert_eq!(failures, 1, "given up once");
    assert!(
        state.failed_items.lock().unwrap().contains("C1"),
        "its end timer is stopped"
    );
    let fade = state.audio_commands.lock().unwrap().iter().any(|c| {
        matches!(c, nbe_engine::audio_control::AudioCommand::TakeItem { sources, ramp_ms, .. } if sources.is_empty() && (*ramp_ms - 500.0).abs() < 1e-3)
    });
    assert!(fade, "its audio fades out over 500 ms");

    // Given up on: no more retries and no second report, however long it
    // plays on, through the dead region to the clip's end.
    let mut f = frame;
    for _ in 0..90 {
        render.settle_read_ahead(f, Duration::ZERO);
        f = (f + 1).min(149);
        std::thread::sleep(Duration::from_millis(33));
    }
    println!(
        "played on to master frame {f}: retries {}, failures {}, reports {}",
        state.clip_retries_total.load(Ordering::SeqCst),
        state.clip_failures_total.load(Ordering::SeqCst),
        state.item_events.lock().unwrap().len()
    );
    assert_eq!(
        state.clip_retries_total.load(Ordering::SeqCst),
        3,
        "no retry after giving up"
    );
    assert_eq!(
        state.clip_failures_total.load(Ordering::SeqCst),
        1,
        "given up once"
    );
    assert_eq!(state.item_events.lock().unwrap().len(), 1, "reported once");
}
