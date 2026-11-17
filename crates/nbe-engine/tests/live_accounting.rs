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
//! publication (as a late render thread would not publish) while frames keep
//! being published and the upload thread keeps uploading. The engine does
//! nothing wrong. Every frame is still uploaded or overwritten, as the test
//! checks once publishing stops. The counts must add up all the same.
//!
//! Publishes and ticks run in lockstep, and both are counted: each step
//! publishes one frame, then draws one tick. No sleeps and no capture thread.
//! The first form fed the mailbox from a thread sleeping 5 ms a frame and
//! required more than 20 frames in the 300 ms late phase. On main's push run at
//! 17e9d3d (runner load1 about 18 on 3 CPUs) that thread delivered 17 frames in
//! the on-time phase and exactly 20 in the late one, and the test failed on its
//! own precondition, with every count adding up (the user's ruling of
//! 2026-11-16 18:40). Here the phases are counts of steps, so what each phase
//! publishes is exact on any machine.

use std::sync::Arc;
use std::time::Instant;

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

/// Steps (one publish, one tick) with the render loop on time.
const ON_TIME_STEPS: u64 = 20;
/// Steps with the render loop late publishing.
const LATE_STEPS: u64 = 30;
/// The most ticks the settling may take before the test calls it stuck. A
/// count, not a time: the upload thread needs one wake to take the last frame.
const SETTLE_TICKS: u64 = 600;

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

/// The capture side and the render loop in lockstep: publish one frame,
/// then draw one tick.
struct Lockstep {
    mailbox: Arc<FrameMailbox>,
    /// Frames published so far (each one's sequence number).
    published: u64,
    /// Ticks drawn so far (the next master frame).
    ticks: u64,
}

impl Lockstep {
    fn publish(&mut self) {
        self.published += 1;
        self.mailbox.publish(LiveFrame {
            width: 64,
            height: 36,
            bgra: vec![0u8; 64 * 36 * 4],
            seq: self.published,
        });
    }

    async fn tick(&mut self, render: &mut RenderLoop, state: &EngineState) {
        run_tick(render, state, self.ticks, None).await;
        self.ticks += 1;
    }

    async fn step(&mut self, render: &mut RenderLoop, state: &EngineState) {
        self.publish();
        self.tick(render, state).await;
    }
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
    let mut ls = Lockstep {
        mailbox,
        published: 0,
        ticks: 0,
    };

    // The render loop on time.
    for _ in 0..ON_TIME_STEPS {
        ls.step(&mut render, &state).await;
    }
    let on_time = counts(&state);

    // The render loop late publishing; frames keep coming and keep being
    // uploaded.
    render.injected_live_publish_hold = true;
    for _ in 0..LATE_STEPS {
        ls.step(&mut render, &state).await;
    }
    let late = counts(&state);

    // No engine fault: with publishing stopped, the render loop on time again
    // and the uploader given ticks to take the last frame, every frame is
    // uploaded or overwritten. Ticks until the counts settle, at most
    // SETTLE_TICKS of them; a wall-clock time is only reported.
    render.injected_live_publish_hold = false;
    let settle_started = Instant::now();
    let mut settle_ticks = 0u64;
    let mut settled = counts(&state);
    while settled.3 != 0 && settle_ticks < SETTLE_TICKS {
        ls.tick(&mut render, &state).await;
        tokio::task::yield_now().await;
        settle_ticks += 1;
        settled = counts(&state);
    }
    let settle_ms = settle_started.elapsed().as_secs_f64() * 1000.0;

    println!(
        "item 3: (in, uploaded, overwritten, unaccounted) on time {on_time:?}; \
         late publishing {late:?}; settled {settled:?} (published {}, ticks {}, \
         settled after {settle_ticks} ticks, {settle_ms:.2} ms)",
        ls.published, ls.ticks
    );
    assert_eq!(
        on_time.0, ON_TIME_STEPS,
        "every frame published on time is counted in: {on_time:?}"
    );
    assert_eq!(
        late.0, on_time.0 + LATE_STEPS,
        "frames kept arriving while late, one a step: {on_time:?} -> {late:?}"
    );
    assert_eq!(
        settled.3, 0,
        "settled, every frame was uploaded or overwritten: {settled:?} after {settle_ticks} ticks"
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
