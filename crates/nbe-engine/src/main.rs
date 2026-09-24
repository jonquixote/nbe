//! nbe-engine binary (Prompt 03 Step 1): boot config, telemetry subscriber, and
//! the control plane channel. Per prompt constraints: clock state is in an
//! Arc<EngineState>; the clock FSM is derived by MainLoop from directives so
//! other tasks can read state.

use nbe_engine::audio_driver;
use nbe_engine::channel::{self, EngineConfig};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, SharedEngineState, SharedOutgoing};
use std::sync::Arc;

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
        // Cadence and tick both live in the library (`nbe_engine::tick`), so
        // tests drive this exact loop. Output costs accumulate into
        // `record_tap_ms` / `stream_tap_ms`, never the render budget: the
        // stream's share of a tick is one bounded `try_send` (the stream
        // thread encodes; the loop never does).
        nbe_engine::tick::run_loop(&mut render, &render_state, house_rate, |_| true).await;
    });

    render_loops
        .run_until(channel::run_forever(cfg, state, outgoing))
        .await;
    Ok(())
}
