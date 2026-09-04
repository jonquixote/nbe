//! nbe-engine binary (Prompt 03 Step 1): boot config, telemetry subscriber, and
//! the control plane channel. Per prompt constraints: clock state is in an
//! Arc<EngineState>; the clock FSM is derived by MainLoop from directives so
//! other tasks can read state.

use nbe_engine::audio_driver;
use nbe_engine::channel::{self, EngineConfig};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, SharedEngineState, SharedOutgoing};
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
    let mut render = RenderLoop::new(state.clone(), None).await?;
    let render_state = state.clone();
    tokio::spawn(async move {
        let frame_budget = Duration::from_secs_f64(1.0 / house_rate as f64);
        let mut next_boundary = Instant::now();
        let mut stopped_frame: u64 = 0;
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
            let _ = render.render_frame(frame, deadline);
            next_boundary += frame_budget;
            // If we fell far behind, resynchronize rather than spiral.
            let now = Instant::now();
            if next_boundary < now {
                next_boundary = now + frame_budget;
            }
        }
    });

    channel::run_forever(cfg, state, outgoing).await;
    Ok(())
}
