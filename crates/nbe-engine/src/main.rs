//! nbe-engine binary (Prompt 03 Step 1): boot config, telemetry subscriber, and
//! the control plane channel. Per prompt constraints: clock state is in an
//! Arc<EngineState>; the clock FSM is derived by MainLoop from directives so
//! other tasks can read state.

use nbe_engine::audio_driver;
use nbe_engine::channel::{self, EngineConfig};
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
            // WU-pipe: one record frame per loop iteration, both paths.
            //
            // Budget honesty is unchanged: the budget pre-check still runs
            // AFTER the draw (it needs the View's measured time) and an
            // over-budget frame still skips with no readback, no handoff and no
            // encode. What the zero-copy path adds is an EARLIER question —
            // "is a free surface available?" — asked before the draw, because
            // on that path the draw goes into the surface and a missing one
            // costs a corrupted frame rather than a skipped one.
            //
            // One lock acquisition resolves both the take's pool and its
            // handoff endpoint, so they cannot describe different takes.
            let recording = *render_state.record_state.lock().unwrap() == RecordState::Recording;
            let (pool, endpoint) = match recording {
                true => {
                    let g = render_state.record_session.lock().unwrap();
                    match g.as_ref() {
                        Some(s) => (s.surface_pool(), Some(s.frame_sender())),
                        None => (None, None),
                    }
                }
                false => (None, None),
            };
            let loan = match nbe_engine::record::begin_tap_frame(
                &mut render,
                pool.as_deref(),
                &render_state.skipped_record_frames,
            ) {
                Ok(loan) => loan,
                // Unreachable today: the pool is built at VIEW_W x VIEW_H by
                // `record.start`, which is the only geometry the View has, so
                // the swap's dimension check cannot fire. Handled rather than
                // unwrapped because "unreachable" is a claim about today's call
                // sites. INTERIM: the record frame is dropped and counted, the
                // View still goes on air. Step 6 replaces this with Option A —
                // the take ends loudly with `E_NO_ZEROCOPY` — which is a
                // decision about mid-take chain loss and is named as new
                // behaviour there, not smuggled in here.
                Err(e) => {
                    tracing::error!(err = %e, "record tap: cannot retarget the View; frame not recorded");
                    render_state
                        .skipped_record_frames
                        .fetch_add(1, Ordering::SeqCst);
                    Default::default()
                }
            };

            let render_started = Instant::now();
            let _ = render.render_frame(frame, deadline);
            let render_elapsed = render_started.elapsed();
            // The View goes back to the built-in target the moment the draw is
            // done, before anything can fail: a retarget left in place would
            // composite the NEXT frame into a surface nobody is holding.
            nbe_engine::record::restore_view(&mut render, &loan);

            if recording {
                if let Some(tx) = endpoint {
                    let feed_ms = nbe_engine::record::end_tap_frame(
                        loan,
                        render_elapsed,
                        deadline,
                        &tx,
                        &render_state.skipped_record_frames,
                        || async {
                            // Timed readback (the only await, and only on the
                            // CPU path): its cost belongs to the record
                            // counter, never the render budget.
                            let started = Instant::now();
                            let rgba = render.readback_view().await;
                            (rgba, started.elapsed())
                        },
                    )
                    .await;
                    *render_state.record_tap_ms.lock().unwrap() += feed_ms;
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
