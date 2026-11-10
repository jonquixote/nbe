//! Step F (the user's rulings of 2026-11-07 and 2026-11-08, 03:45): where a
//! draw's time goes, log-only. `render_frame` times its phases into
//! `FrameReport::phases`; the R21 line carries the View's (`tick.rs`'s lib
//! test pins its fields); and each run's first draw gets one info line of
//! every phase. These guards pin the two claims a reader of the soak's log
//! leans on: one first-draw line per run, naming that run's first drawn
//! frame; and phases measured inside the draw they describe. Nothing here
//! reads a wall-clock limit (R39): the second guard compares two durations of
//! the same draw, which a stall lengthens together.
//!
//! The production loop (`tick::run_loop`) on a real `RenderLoop`, as in
//! `run_edges.rs`, with its log captured through tests/common/capture.rs.

use std::sync::Arc;
use std::time::Duration;

use nbe_engine::render::RenderLoop;
use nbe_engine::state::EngineState;
use nbe_engine::tick::{run_loop, TickReport};

#[path = "common/capture.rs"]
mod capture;
use capture::captured;

const RATE: u32 = 30;
const FIRST_DRAW: &str = "render phases: the run's first draw";

/// The engine state and its render loop, or a loud skip without an adapter.
async fn rig_or_skip() -> Option<(Arc<EngineState>, RenderLoop)> {
    let state = Arc::new(EngineState::new(RATE));
    match RenderLoop::new(state.clone()).await {
        Ok(render) => Some((state, render)),
        Err(e) => {
            eprintln!("SKIP: no GPU adapter for the render loop ({e})");
            None
        }
    }
}

/// A field's value on a log line, or a panic naming the line.
fn field(line: &str, name: &str) -> f64 {
    let at = line
        .find(&format!(" {name}="))
        .unwrap_or_else(|| panic!("{name} missing: {line}"));
    line[at + name.len() + 2..]
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{name} unreadable: {line}"))
}

/// Two runs on the real loop: started at tick 5, stopped at tick 20, started
/// again at tick 25, ended at tick 40.
async fn two_runs(state: &EngineState, render: &mut RenderLoop) -> (Vec<TickReport>, String) {
    let mut reports = Vec::new();
    let ((), log) = captured(run_loop(render, state, RATE, |r| {
        reports.push(*r);
        match reports.len() {
            5 | 25 => state.start_clock(),
            20 => state.clock.lock().unwrap().stop(),
            _ => {}
        }
        reports.len() < 40
    }))
    .await;
    (reports, log)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_run_s_first_draw_logs_one_phases_line_naming_its_frame() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    let (reports, log) = two_runs(&state, &mut render).await;
    // Each run's first drawn frame, from the loop's own reports: the first
    // running tick after a stopped one.
    let mut firsts = Vec::new();
    let mut running = false;
    for r in &reports {
        match (r.master_frame, running) {
            (Some(f), false) => {
                firsts.push(f);
                running = true;
            }
            (None, _) => running = false,
            _ => {}
        }
    }
    assert_eq!(firsts.len(), 2, "precondition: the loop drew two runs");
    let lines: Vec<&str> = log.lines().filter(|l| l.contains(FIRST_DRAW)).collect();
    let frames: Vec<u64> = lines
        .iter()
        .map(|l| field(l, "master_frame") as u64)
        .collect();
    eprintln!("first draws: runs began at {firsts:?}; lines name {frames:?}");
    assert_eq!(
        frames,
        firsts,
        "one first-draw line per run, naming the run's first drawn frame:\n{}",
        lines.join("\n")
    );
    for line in &lines {
        for name in [
            "render_ms",
            "previous_tick_ms",
            "woke_late_ms",
            "sync_ms",
            "reacquire_ms",
            "read_ahead_ms",
            "preview_build_ms",
            "preview_encode_ms",
            "preview_finish_ms",
            "preview_submit_ms",
            "view_build_ms",
            "view_encode_ms",
            "view_finish_ms",
            "view_submit_ms",
        ] {
            field(line, name);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_drawn_tick_s_phases_are_measured_inside_its_draw() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    let (reports, _log) = two_runs(&state, &mut render).await;
    let drawn: Vec<&TickReport> = reports
        .iter()
        .filter(|r| r.master_frame.is_some())
        .collect();
    assert!(
        drawn.len() >= 20,
        "precondition: the loop drew, {} ticks",
        drawn.len()
    );
    for r in drawn {
        let p = r.render_phases;
        let inside = p.sync + p.reacquire + p.read_ahead + p.preview.total() + p.view.total();
        // The phases are consecutive parts of `render_frame`, and `render` is
        // measured around it, so their sum never exceeds it (1 µs for the
        // clock's own rounding).
        assert!(
            inside <= r.render + Duration::from_micros(1),
            "frame {:?}: the phases ({inside:?}) exceed the draw they are part of ({:?}): {p:?}",
            r.master_frame,
            r.render
        );
        // The View drew and submitted on every running tick, so its draw's
        // phases are recorded, not left at zero.
        assert!(
            p.view.encode > Duration::ZERO && p.view.submit > Duration::ZERO,
            "frame {:?}: the View's phases were not recorded: {p:?}",
            r.master_frame
        );
    }
}
