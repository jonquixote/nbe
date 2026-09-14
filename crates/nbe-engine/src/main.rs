//! nbe-engine binary (Prompt 03 Step 1): boot config, telemetry subscriber, and
//! the control plane channel. Per prompt constraints: clock state is in an
//! Arc<EngineState>; the clock FSM is derived by MainLoop from directives so
//! other tasks can read state.

use nbe_engine::audio_driver;
use nbe_engine::channel::{self, EngineConfig};
use nbe_engine::encode::EncodeSession;
use nbe_engine::record::{feed_record_frame, AudioTap};
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
    // The render/record loop below holds the hardware encoder across
    // frames, and that handle is !Send (a raw VideoToolbox session pointer)
    // — so the loop runs as a thread-local task on a LocalSet instead of
    // `tokio::spawn` (whose `Send` bound forbids it). No new threads: the
    // set is driven on this thread, and the render budget/deadline path is
    // untouched by the choice of spawner.
    let render_loops = tokio::task::LocalSet::new();
    render_loops.spawn_local(async move {
        let frame_budget = Duration::from_secs_f64(1.0 / house_rate as f64);
        let mut next_boundary = Instant::now();
        let mut stopped_frame: u64 = 0;
        // WU-tap record feed (additive only): the encoder, tap, and counters
        // below never enter the render budget — `render_frame` keeps its own
        // deadline check UNCHANGED, and the feed runs after it, accumulating
        // into the engine-state `record_tap_ms` counter. Over budget the
        // record frame is SKIPPED before the readback (record degrades, View
        // never). The feed owns the ONE live encoder (opened at VIEW_W/H on
        // the first fed frame); the session stays metadata-only until the
        // feed captures the real SPS/PPS from the first keyframe.
        //
        // Audio note: the tap here drains whatever the record-start wiring
        // attached via `AudioGraph::set_record_tap` (live-graph attach needs
        // state/audio-driver/directive changes, a later unit); the video leg
        // is live from this work unit.
        let mut record_encoder: Option<EncodeSession> = None;
        let mut record_tap: Option<std::sync::Arc<AudioTap>> = None;
        let mut record_setup_failed = false;
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
            // WU-tap: feed one record frame after the deadline check above.
            // Budget honesty: the pre-check runs BEFORE the readback — an
            // over-budget frame skips (and counts) with no readback await, no
            // encode, no pushes.
            let recording = *render_state.record_state.lock().unwrap() == RecordState::Recording;
            if recording {
                let over_budget = deadline.map(|b| render_elapsed >= b).unwrap_or(false);
                if over_budget {
                    render_state
                        .skipped_record_frames
                        .fetch_add(1, Ordering::SeqCst);
                } else if !record_setup_failed {
                    if record_tap.is_none() {
                        record_tap = Some(std::sync::Arc::new(AudioTap::new()));
                    }
                    // Timed readback (the only await): its cost belongs to the
                    // record counter, never the render budget — the feed folds
                    // it into `outcome.feed_ms`.
                    let readback_started = Instant::now();
                    let rgba = render.readback_view().await;
                    let readback_elapsed = readback_started.elapsed();
                    if let Some(tap) = record_tap.clone() {
                        if let Some(session) = render_state.record_session.lock().unwrap().as_mut()
                        {
                            let outcome = feed_record_frame(
                                &rgba,
                                &mut record_encoder,
                                session,
                                &tap,
                                render_elapsed,
                                deadline,
                                readback_elapsed,
                            );
                            if outcome.setup_failed {
                                record_setup_failed = true;
                            }
                            *render_state.record_tap_ms.lock().unwrap() += outcome.feed_ms;
                            if outcome.skipped {
                                render_state
                                    .skipped_record_frames
                                    .fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                }
                tracing::debug!(
                    record_tap_ms = *render_state.record_tap_ms.lock().unwrap(),
                    skipped_record_frames =
                        render_state.skipped_record_frames.load(Ordering::SeqCst),
                    "record feed tick"
                );
            } else {
                record_encoder = None;
                record_tap = None;
                record_setup_failed = false;
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
