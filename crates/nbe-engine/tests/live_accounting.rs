//! Item 3 of the user's ruling of 2026-11-16 00:25: the camera's frame counts
//! in telemetry add up, even while the render loop is late.
//!
//! The camera e2e checks `framesIn = framesUploaded + framesOverwritten`, give
//! or take 2 frames in flight (one in the mailbox, one being uploaded). Main's
//! push run at 8b482fe read 3 (in 221 = uploaded 143 + overwritten 75). Its
//! rerun read 0. The reading: `framesIn` and `framesOverwritten` are the
//! mailbox's live counters, but `framesUploaded` was the render thread's copy,
//! published once a frame. A late render frame left it behind, and frames that
//! were already uploaded counted as in flight.
//!
//! This makes that deterministic. A seam holds the render loop's per-frame
//! publication (as a late render thread would not publish) while a capture
//! side keeps publishing and the upload thread keeps uploading. The engine
//! does nothing wrong. Every frame is still uploaded or overwritten, as the
//! test checks once the capture side stops. The counts must add up all the same.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nbe_engine::live::{CaptureHandle, FrameMailbox, LiveFrame};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::EngineState;
use nbe_engine::tick::run_tick;

async fn render_loop_or_skip() -> Option<(Arc<EngineState>, RenderLoop)> {
    let state = Arc::new(EngineState::new(30));
    match RenderLoop::new(state.clone()).await {
        Ok(r) => Some((state, r)),
        Err(e) => {
            eprintln!("SKIP: no GPU adapter on this machine ({e})");
            None
        }
    }
}

/// A capture that runs nowhere; the test publishes into its mailbox itself.
struct Fed;
impl CaptureHandle for Fed {}

const SOURCE: &str = "camera:accounting";

/// (frames in, uploaded, overwritten) as telemetry reports them now, and
/// what that leaves unaccounted for.
fn counts(state: &EngineState) -> (u64, u64, u64, i64) {
    let line = state
        .live
        .lines()
        .into_iter()
        .find(|l| l.source == SOURCE)
        .expect("telemetry reports the source");
    let (i, u, o) = (
        line.frames_in,
        line.frames_uploaded,
        line.frames_overwritten,
    );
    (i, u, o, i as i64 - u as i64 - o as i64)
}

#[tokio::test]
async fn the_camera_counts_add_up_while_the_render_loop_is_late_publishing() {
    let Some((state, mut render)) = render_loop_or_skip().await else {
        return;
    };
    let mailbox = Arc::new(FrameMailbox::default());
    state
        .live
        .add_video_source(SOURCE.into(), mailbox.clone(), Box::new(Fed));

    // The capture side: a small frame every 5 ms until told to stop.
    let stop = Arc::new(AtomicBool::new(false));
    let feeder = {
        let (stop, mailbox) = (stop.clone(), mailbox.clone());
        std::thread::spawn(move || {
            let mut seq = 0u64;
            while !stop.load(Ordering::SeqCst) {
                seq += 1;
                mailbox.publish(LiveFrame {
                    width: 64,
                    height: 36,
                    bgra: vec![0u8; 64 * 36 * 4],
                    seq,
                });
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };

    // The render loop on time: a tick every 10 ms for 200 ms.
    let mut frame = 0u64;
    for _ in 0..20 {
        run_tick(&mut render, &state, frame, None).await;
        frame += 1;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let on_time = counts(&state);

    // The render loop late publishing for 300 ms; frames keep coming and
    // keep being uploaded.
    render.injected_live_publish_hold = true;
    for _ in 0..30 {
        run_tick(&mut render, &state, frame, None).await;
        frame += 1;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let late = counts(&state);

    // No engine fault: with the capture side stopped and the uploader given
    // time to take the last frame, every frame is uploaded or overwritten.
    render.injected_live_publish_hold = false;
    stop.store(true, Ordering::SeqCst);
    feeder.join().unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    run_tick(&mut render, &state, frame, None).await;
    let settled = counts(&state);

    println!(
        "item 3: (in, uploaded, overwritten, unaccounted) on time {on_time:?}; late publishing {late:?}; settled {settled:?}"
    );
    assert!(
        late.0 > on_time.0 + 20,
        "frames kept arriving while late: {on_time:?} -> {late:?}"
    );
    assert_eq!(
        settled.3, 0,
        "settled, every frame was uploaded or overwritten: {settled:?}"
    );
    assert!(
        (0..=2).contains(&on_time.3),
        "on time, at most 2 frames in flight: {on_time:?}"
    );
    assert!(
        (0..=2).contains(&late.3),
        "with the render loop late, still at most 2 frames in flight: {late:?}"
    );
}
