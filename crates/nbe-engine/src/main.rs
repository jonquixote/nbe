//! nbe-engine binary (Prompt 03 Step 1): boot config, telemetry subscriber, and
//! the control plane channel. Per prompt constraints: clock state is in an
//! Arc<EngineState>; the clock FSM is derived by MainLoop from directives so
//! other tasks can read state.

use nbe_engine::audio_driver;
use nbe_engine::channel::{self, EngineConfig};
use nbe_engine::record::{handoff_record_frame, should_skip_record_frame};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, RecordState, SharedEngineState, SharedOutgoing};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let ctrl =
        std::env::var("NBE_CP_URL").unwrap_or_else(|_| "ws://127.0.0.1:8462/nbe/v0.3".into());
    let token = std::env::var("NBE_RENDER_TOKEN")
        .map_err(|_| anyhow::anyhow!("NBE_RENDER_TOKEN required"))?;
    let house_rate: u32 = std::env::var("NBE_HOUSE_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let state: SharedEngineState = Arc::new(EngineState::new(house_rate));
    let outgoing: SharedOutgoing = Arc::new(Default::default());

    let cfg = EngineConfig {
        control_plane_url: ctrl,
        token,
        house_rate,
        telemetry_interval_ms: 1000,
    };

    // The audio driver runs on its own task, on the audio cadence — never in
    // the render loop (SPEC §8.9, §7.13). It owns the graph, drains the
    // intents the directive path publishes, and publishes the v0.3.3 audio
    // telemetry fields. Without it the graph is a mechanism nothing drives.
    //
    // Started before the GPU: audio does not depend on wgpu, and a slow or
    // failing device init must not silence the show.
    //
    // The sink is the null sink: device glue is the recorded deferral in
    // agents/prompts/06-audio-graph.md. Everything above the sink is real.
    audio_driver::spawn(state.clone(), house_rate);

    // The render loop runs on its own task, driven by master-clock frame
    // boundaries — not a spin. It renders even while the clock is STOPPED so
    // the operator always has a picture (the fallback slate).
    let mut render = RenderLoop::new(state.clone()).await?;
    let render_state = state.clone();
    // The render/record loop below hands frames to the dedicated record
    // thread over a bounded channel; the thread owns the hardware encoder
    // across takes, and that handle is !Send (a raw VideoToolbox session
    // pointer) — so the loop runs as a thread-local task on a LocalSet
    // instead of `tokio::spawn` (whose `Send` bound forbids it). No new
    // threads: the set is driven on this thread, and the render
    // budget/deadline path is untouched by the choice of spawner.
    let render_loops = tokio::task::LocalSet::new();
    render_loops.spawn_local(async move {
        let frame_budget = Duration::from_secs_f64(1.0 / house_rate as f64);
        let mut next_boundary = Instant::now();
        let mut stopped_frame: u64 = 0;
        // WU-pipe record handoff (additive only): the budget pre-check and
        // the handoff below never enter the render budget — `render_frame`
        // keeps its own deadline check UNCHANGED, and the record cost
        // (readback + handoff) accumulates into the engine-state
        // `record_tap_ms` counter. Over budget the record frame is SKIPPED
        // BEFORE the readback (record degrades, View never); on a saturated
        // handoff channel the frame is SHED (same counter). Both count in
        // `skipped_record_frames`.
        //
        // Audio note: the record thread drains the shared tap that
        // `record.start` published and the audio driver attached to the live
        // graph; the loop never touches audio.
        loop {
            let now = Instant::now();
            if next_boundary > now {
                tokio::time::sleep(next_boundary - now).await;
            }
            let (frame, deadline) = match render_state.master_frame() {
                // RUNNING: the master clock owns the frame number and the
                // deadline is real.
                Some(f) => (f, Some(frame_budget)),
                // STOPPED: still render, but a missed deadline is meaningless
                // when no show clock is running, so nothing is counted.
                None => {
                    stopped_frame = stopped_frame.wrapping_add(1);
                    (stopped_frame, None)
                }
            };
            let render_started = Instant::now();
            let _ = render.render_frame(frame, deadline);
            let render_elapsed = render_started.elapsed();
            // WU-pipe: hand one record frame after the deadline check above.
            // Budget honesty: the pre-check runs BEFORE the readback — an
            // over-budget frame skips (and counts) with no readback await, no
            // handoff, no encode. A live handoff is a non-blocking `try_send`;
            // a saturated thread sheds (and counts) instead of slowing View.
            let recording = *render_state.record_state.lock().unwrap() == RecordState::Recording;
            if recording {
                if should_skip_record_frame(render_elapsed, deadline) {
                    render_state
                        .skipped_record_frames
                        .fetch_add(1, Ordering::SeqCst);
                } else {
                    // Resolve the handoff endpoint without holding the lock
                    // across the readback await below.
                    let endpoint = render_state
                        .record_session
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|s| s.frame_sender());
                    if let Some(tx) = endpoint {
                        // Timed readback (the only await): its cost belongs to
                        // the record counter, never the render budget — the
                        // handoff folds it into `outcome.feed_ms`.
                        let readback_started = Instant::now();
                        let rgba = render.readback_view().await;
                        let readback_elapsed = readback_started.elapsed();
                        let outcome = handoff_record_frame(rgba, &tx, readback_elapsed);
                        *render_state.record_tap_ms.lock().unwrap() += outcome.feed_ms;
                        if !outcome.sent {
                            render_state
                                .skipped_record_frames
                                .fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
                tracing::debug!(
                    record_tap_ms = *render_state.record_tap_ms.lock().unwrap(),
                    skipped_record_frames =
                        render_state.skipped_record_frames.load(Ordering::SeqCst),
                    "record handoff tick"
                );
            }
            next_boundary += frame_budget;
            // If we fell far behind, resynchronize rather than spiral.
            let now = Instant::now();
            if next_boundary < now {
                next_boundary = now + frame_budget;
            }
        }
    });

    render_loops
        .run_until(channel::run_forever(cfg, state, outgoing))
        .await;
    Ok(())
}
