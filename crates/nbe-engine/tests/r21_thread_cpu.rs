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

/// The A/B of the user's ruling of 2026-11-15 17:30 (log-only): each bus
/// phase's CPU is measured over its own span. No phase's CPU exceeds its
/// wall time, the phases' CPU never exceeds the draw's, the View's four are
/// not all zero over a few real draws, and the render thread's system time
/// is part of its CPU, not beside it.
#[tokio::test]
async fn each_phase_s_cpu_is_measured_over_its_own_span() {
    let Some((state, mut render)) = render_loop_or_skip().await else {
        return;
    };
    run_tick(&mut render, &state, 0, None).await;
    let slack = Duration::from_millis(2);
    let mut view_cpu_seen = Duration::ZERO;
    for frame in 1..=10 {
        let r = run_tick(&mut render, &state, frame, None).await;
        let p = r.render_phases;
        for (name, wall, cpu) in [
            ("preview build", p.preview.build, p.preview.build_cpu),
            ("preview encode", p.preview.encode, p.preview.encode_cpu),
            ("preview finish", p.preview.finish, p.preview.finish_cpu),
            ("preview submit", p.preview.submit, p.preview.submit_cpu),
            ("view build", p.view.build, p.view.build_cpu),
            ("view encode", p.view.encode, p.view.encode_cpu),
            ("view finish", p.view.finish, p.view.finish_cpu),
            ("view submit", p.view.submit, p.view.submit_cpu),
            ("sync", p.sync, p.cpu.sync),
            ("reacquire", p.reacquire, p.cpu.reacquire),
        ] {
            assert!(
                cpu <= wall + slack,
                "frame {frame}: {name}'s CPU {cpu:?} above its wall {wall:?}"
            );
        }
        let phases_cpu = p.cpu.sync
            + p.cpu.reacquire
            + p.cpu.read_ahead
            + p.preview.build_cpu
            + p.preview.encode_cpu
            + p.preview.finish_cpu
            + p.preview.submit_cpu
            + p.view.build_cpu
            + p.view.encode_cpu
            + p.view.finish_cpu
            + p.view.submit_cpu;
        assert!(
            phases_cpu <= r.render_cpu + slack,
            "frame {frame}: the phases' CPU {phases_cpu:?} above the draw's {:?}",
            r.render_cpu
        );
        assert!(
            r.render_sys <= r.render_cpu + slack,
            "frame {frame}: system time {:?} above the thread's CPU {:?}",
            r.render_sys,
            r.render_cpu
        );
        view_cpu_seen +=
            p.view.build_cpu + p.view.encode_cpu + p.view.finish_cpu + p.view.submit_cpu;
    }
    assert!(
        view_cpu_seen > Duration::ZERO,
        "ten real View draws measured no CPU in any of the View's phases"
    );
}
