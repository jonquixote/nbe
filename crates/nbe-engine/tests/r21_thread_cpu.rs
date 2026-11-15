//! R21's diagnostic (the user's ruling of 2026-11-14): the render thread's
//! own CPU time beside the wall time, on the R21 line and on the frame a live
//! source joins or leaves.
//!
//! The question it answers is whether a slow phase worked or waited. These
//! tests drive the real render loop through `run_tick` and check that the CPU
//! figure is measured, not copied from the wall clock: a draw that sleeps (the
//! View's injected delay, a stand-in for a descheduled thread) costs wall time
//! and almost no CPU, while the CPU figure is never zero and never above the
//! wall time.

#[path = "common/capture.rs"]
mod capture;

use std::sync::Arc;
use std::time::Duration;

use capture::captured;
use nbe_engine::live::{CaptureHandle, FrameMailbox};
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

/// A video source that runs nowhere: the test publishes nothing, so the
/// upload thread the render loop starts for it only waits.
struct Idle;
impl CaptureHandle for Idle {}

#[tokio::test]
async fn the_frame_a_live_source_joins_or_leaves_logs_the_resync_wall_and_cpu() {
    let Some((state, mut render)) = render_loop_or_skip().await else {
        return;
    };
    run_tick(&mut render, &state, 0, None).await;

    let (joined, log) = captured(async {
        state.live.add_video_source(
            "guest:t1".into(),
            Arc::new(FrameMailbox::default()),
            Box::new(Idle),
        );
        run_tick(&mut render, &state, 1, None).await
    })
    .await;
    let r = joined
        .render_phases
        .live_resync
        .expect("the frame after a source joined resyncs the upload threads");
    assert_eq!((r.uploaders_before, r.uploaders_after), (0, 1));
    assert!(r.cpu <= r.wall + Duration::from_millis(2), "{r:?}");
    assert!(
        log.contains("live sources changed")
            && log.contains("resync_cpu_ms")
            && log.contains("uploaders_after=1"),
        "the join's line, with the resync's CPU beside its wall time:\n{log}"
    );

    let quiet = run_tick(&mut render, &state, 2, None).await;
    assert!(
        quiet.render_phases.live_resync.is_none(),
        "no change, no resync"
    );

    let (left, log) = captured(async {
        assert!(state.live.remove_video_source("guest:t1"));
        run_tick(&mut render, &state, 3, None).await
    })
    .await;
    let r = left
        .render_phases
        .live_resync
        .expect("the frame after a source left resyncs the upload threads");
    println!(
        "R21 diagnostic: a leaving source's upload thread stopped on the render thread in {:?} wall, {:?} CPU (frame render {:?} wall, {:?} CPU)",
        r.wall, r.cpu, left.render, left.render_cpu
    );
    assert_eq!((r.uploaders_before, r.uploaders_after), (1, 0));
    assert!(r.cpu <= r.wall + Duration::from_millis(2), "{r:?}");
    let line = log
        .lines()
        .find(|l| l.contains("live sources changed") && l.contains("uploaders_after=0"))
        .unwrap_or_else(|| panic!("the leave's line:\n{log}"));
    for (name, want) in [
        ("resync_ms", r.wall),
        ("resync_cpu_ms", r.cpu),
        ("render_ms", left.render),
        ("render_cpu_ms", left.render_cpu),
    ] {
        assert!(
            (field_ms(line, name) - want.as_secs_f64() * 1000.0).abs() < 1e-6,
            "{name} is the measured {want:?}: {line}"
        );
    }

    // A leave's resync joins the leaving source's upload thread, which waits
    // up to 50 ms on its mailbox before it sees the stop flag. That is a
    // wait: its wall time shows and its render-thread CPU does not. Where
    // the thread is in its wait is chance, so a source joins and leaves
    // until one leave's join waited 20 ms or more (leaving one tick after
    // joining, nearly every try does).
    let mut waited = None;
    for i in 0..20u64 {
        let f = 4 + 2 * i;
        state.live.add_video_source(
            "guest:t1".into(),
            Arc::new(FrameMailbox::default()),
            Box::new(Idle),
        );
        run_tick(&mut render, &state, f, None).await;
        assert!(state.live.remove_video_source("guest:t1"));
        let r = run_tick(&mut render, &state, f + 1, None)
            .await
            .render_phases
            .live_resync
            .expect("the frame after a source left resyncs the upload threads");
        if r.wall >= Duration::from_millis(20) {
            waited = Some(r);
            break;
        }
    }
    let r = waited.expect("in 20 leaves, one join waited 20 ms or more");
    assert!(
        r.cpu * 2 < r.wall,
        "a join that waited is wall time, not render-thread CPU: {r:?}"
    );
}

/// A `name=value` field of a log line, in milliseconds.
fn field_ms(line: &str, name: &str) -> f64 {
    let key = format!(" {name}=");
    let at = line
        .find(&key)
        .unwrap_or_else(|| panic!("{name} missing: {line}"));
    line[at + key.len()..]
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{name} unreadable: {line}"))
}
