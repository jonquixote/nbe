//! R21's diagnostic (the user's ruling of 2026-11-14): the render thread's
//! own CPU time beside the wall time, on the R21 line. Ported to the A side
//! of the A/B (the user's ruling of 2026-11-15 17:30): this commit has no
//! live sources, so the live-resync test is not here.
//!
//! The question it answers is whether a slow phase worked or waited. These
//! tests drive the real render loop through `run_tick` and check that the CPU
//! figure is measured, not copied from the wall clock: a draw that sleeps (the
//! View's injected delay, a stand-in for a descheduled thread) costs wall time
//! and almost no CPU, while the CPU figure is never zero and never above the
//! wall time.

use std::sync::Arc;
use std::time::Duration;

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

#[tokio::test]
async fn a_draw_that_waits_costs_wall_time_not_render_thread_cpu() {
    let Some((state, mut render)) = render_loop_or_skip().await else {
        return;
    };
    // A run's first draw is slow for reasons of its own (the frame-1 pattern).
    run_tick(&mut render, &state, 0, None).await;

    render.injected_view_delay = Some(Duration::from_millis(80));
    let waited = run_tick(&mut render, &state, 1, None).await;
    println!(
        "R21 diagnostic: an 80 ms wait in the View's draw: render {:?} wall, {:?} CPU; the View's CPU {:?}",
        waited.render, waited.render_cpu, waited.render_phases.cpu.view
    );
    assert!(
        waited.render >= Duration::from_millis(80),
        "the injected wait is inside render's wall time: {:?}",
        waited.render
    );
    assert!(
        waited.render_cpu > Duration::ZERO,
        "the render thread did some work, so its CPU time is not zero"
    );
    assert!(
        waited.render_cpu + Duration::from_millis(50) < waited.render,
        "an 80 ms wait must not count as render-thread CPU: wall {:?}, CPU {:?}",
        waited.render,
        waited.render_cpu
    );
    assert!(
        waited.render_phases.cpu.view < Duration::from_millis(40),
        "the View's CPU span holds the wait and stays low: {:?}",
        waited.render_phases.cpu.view
    );

    render.injected_view_delay = None;
    let worked = run_tick(&mut render, &state, 2, None).await;
    assert!(
        worked.render_cpu <= worked.render + Duration::from_millis(2),
        "a thread cannot run longer than the wall clock: wall {:?}, CPU {:?}",
        worked.render,
        worked.render_cpu
    );
    let p = worked.render_phases.cpu;
    assert!(
        p.read_ahead + p.preview + p.view <= worked.render_cpu + Duration::from_millis(2),
        "the phases' CPU fits inside the whole render's: {p:?} of {:?}",
        worked.render_cpu
    );
}
