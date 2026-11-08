//! R33's run edges (#77's precision note 1): a skip was counted only between
//! two running ticks, so the master frames at a run's edges went uncounted —
//! a stall across the clock's start, or a wake into STOPPED. Each such frame
//! is a master frame never drawn, so a dropped frame (§10.2, ruled
//! 2026-10-28). The production loop (`tick::run_loop`) on a real
//! `RenderLoop`, as in `r33_cadence.rs`; the stall is the loop's own thread
//! held inside `on_tick`, where a directive would apply.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use nbe_engine::render::RenderLoop;
use nbe_engine::state::EngineState;
use nbe_engine::tick::{run_loop, TickReport};

const RATE: u32 = 30;

fn budget() -> Duration {
    Duration::from_secs_f64(1.0 / RATE as f64)
}

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

/// The master frames skipped between two running ticks of the reports: what
/// the loop counted before the edges were.
fn in_run_skips(reports: &[TickReport]) -> u64 {
    let frames: Vec<u64> = reports.iter().filter_map(|r| r.master_frame).collect();
    frames
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0] + 1))
        .sum()
}

/// One global thread-keyed capture (R52, #89): never a scoped subscriber.
/// These tests assert presences only (each edge's line), so the line itself
/// proves the capture saw it.
#[path = "common/capture.rs"]
mod capture;
use capture::captured;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stall_across_the_clock_s_start_counts_the_frames_never_drawn() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    let mut reports = Vec::new();
    let ((), log) = captured(run_loop(&mut render, &state, RATE, |r| {
        reports.push(*r);
        if reports.len() == 5 {
            // The clock starts, and the loop's thread is held two and a half
            // frames before its first running tick.
            state.clock.lock().unwrap().start();
            std::thread::sleep(budget() * 5 / 2);
        }
        reports.len() < 40
    }))
    .await;

    let first = reports
        .iter()
        .find_map(|r| r.master_frame)
        .expect("the clock ran");
    let skipped = state.skipped_master_frames.load(Ordering::SeqCst);
    let in_run = in_run_skips(&reports);
    eprintln!(
        "run edges: the run's first tick drew {first}; skips {skipped} ({in_run} in the run)"
    );
    assert!(
        first >= 1,
        "the stall left no frame behind the run's first tick"
    );
    assert_eq!(
        skipped,
        first + in_run,
        "frames 0..={} were never drawn at the run's start and must be counted",
        first - 1
    );
    assert!(state.dropped_frames_total.load(Ordering::SeqCst) >= skipped);
    assert!(
        log.contains(&format!(
            "R33: master frames 0..={} were never drawn at the run's start",
            first - 1
        )),
        "{log}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wake_into_stopped_counts_the_frames_the_clock_passed_before_it_stopped() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    state.clock.lock().unwrap().start();
    let mut reports = Vec::new();
    let ((), log) = captured(run_loop(&mut render, &state, RATE, |r| {
        reports.push(*r);
        if reports.len() == 5 {
            // The loop's thread is held three and a half frames, then the
            // clock stops: the loop's next wake reads STOPPED. Early in the
            // run, so that however the machine runs at most four skips come
            // before the edge, and its line is inside the five a second.
            std::thread::sleep(budget() * 7 / 2);
            state.clock.lock().unwrap().stop();
        }
        reports.len() < 15
    }))
    .await;

    let last = reports[4].master_frame.expect("tick 5 ran on the clock");
    let (_, at_stop) = state
        .last_stopped_run()
        .expect("the stopped run is remembered");
    // The frame the clock stopped in is not dropped: it never reached its
    // successor's boundary.
    let edge = at_stop.saturating_sub(last + 1);
    let skipped = state.skipped_master_frames.load(Ordering::SeqCst);
    let in_run = in_run_skips(&reports);
    eprintln!("run edges: last drawn {last}, the clock stopped in {at_stop}; skips {skipped} ({in_run} in the run)");
    assert!(
        edge >= 1,
        "the stall left no frame before the stop: {last} then {at_stop}"
    );
    assert_eq!(
        skipped,
        in_run + edge,
        "frames {}..={} were never drawn before the clock stopped and must be counted",
        last + 1,
        at_stop - 1
    );
    assert!(
        log.contains(&format!(
            "R33: master frames {}..={} were never drawn before the clock stopped",
            last + 1,
            at_stop - 1
        )),
        "{log}"
    );
    // The stop's edge lands in the run it ends.
    let run_line = log
        .lines()
        .find(|l| l.contains("cadence: run ended after 5 ticks"))
        .unwrap_or_else(|| panic!("{log}"));
    assert!(
        run_line.contains(&format!("skips {}", in_run + edge)),
        "{run_line}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_and_a_stop_count_at_their_edges_only_what_the_clock_passed() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    let mut reports = Vec::new();
    run_loop(&mut render, &state, RATE, |r| {
        reports.push(*r);
        match reports.len() {
            5 => state.clock.lock().unwrap().start(),
            25 => state.clock.lock().unwrap().stop(),
            _ => {}
        }
        reports.len() < 30
    })
    .await;
    let first = reports
        .iter()
        .find_map(|r| r.master_frame)
        .expect("the clock ran");
    let last = reports[24].master_frame.expect("tick 25 ran on the clock");
    let (_, at_stop) = state
        .last_stopped_run()
        .expect("the stopped run is remembered");
    // No stall leaves both edges empty. A machine that stalls across one
    // leaves frames there, and they are counted: by the clock, so that the two
    // read alike (R39). A frame counted that the clock did not pass is a red.
    let (start_edge, stop_edge) = (first, at_stop.saturating_sub(last + 1));
    let in_run = in_run_skips(&reports);
    eprintln!("run edges: {start_edge} at the start, {in_run} in the run, {stop_edge} at the stop");
    assert_eq!(
        state.skipped_master_frames.load(Ordering::SeqCst),
        start_edge + in_run + stop_edge,
        "the edges count what the clock passed there and nothing else"
    );
}

/// The stop edge's epoch check, pinned (the second key's scenario on #79):
/// the loop draws on run E1; between two ticks E1 stops past the frame the
/// loop drew, E2 starts, runs on and stops too; the next tick wakes into
/// STOPPED. The stop it reads is E2's, not the run the loop last drew on, so
/// there is no edge: nothing is counted. Without the epoch check the frames
/// from E1's last drawn frame to E2's stop would be counted as one span
/// mixing two runs.
///
/// R39: no wall-clock limit. The preconditions are read from the clock
/// itself (each run reaches the frames the scenario needs before it stops),
/// and a sleep only lengthens, so they hold however the machine runs; the
/// test says so loudly if they do not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_from_another_run_is_no_edge_of_the_run_last_drawn() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    /// Sleep on this thread until the running clock reads `frame` or later.
    fn run_to(state: &EngineState, frame: u64) {
        let boundary = state
            .clock
            .lock()
            .unwrap()
            .boundary(frame)
            .expect("the clock is running");
        let now = std::time::Instant::now();
        if boundary > now {
            std::thread::sleep(boundary - now);
        }
    }
    let mut reports: Vec<TickReport> = Vec::new();
    let mut scenario: Option<(u64, u64, u64, u64, u64)> = None; // last, E1 stop, E2 stop, skipped, dropped
    let mut after: Option<(Option<u64>, u64, u64)> = None; // the next tick's frame, skipped, dropped
    run_loop(&mut render, &state, RATE, |r| {
        reports.push(*r);
        match reports.len() {
            3 => state.clock.lock().unwrap().start(),
            n if n >= 5 && scenario.is_none() => {
                if let Some(last) = r.master_frame {
                    let e1 = state.clock.lock().unwrap().epoch().expect("E1 runs");
                    // E1 runs past the frame after the one drawn, then stops.
                    run_to(&state, last + 2);
                    state.clock.lock().unwrap().stop();
                    let (stopped, e1_stop) = state.last_stopped_run().expect("E1 stopped");
                    assert_eq!(stopped, e1, "the stop remembered is E1's");
                    // E2 starts, runs past E1's span, and stops.
                    state.clock.lock().unwrap().start();
                    let e2 = state.clock.lock().unwrap().epoch().expect("E2 runs");
                    assert_ne!(e2, e1, "E2 is a new run");
                    run_to(&state, last + 5);
                    state.clock.lock().unwrap().stop();
                    let (stopped, e2_stop) = state.last_stopped_run().expect("E2 stopped");
                    assert_eq!(stopped, e2, "the stop remembered is now E2's");
                    assert!(
                        e1_stop > last + 1 && e2_stop > last + 1,
                        "precondition: both runs stopped past the frame after the one drawn \
                         (drawn {last}, E1 stopped in {e1_stop}, E2 in {e2_stop})"
                    );
                    scenario = Some((
                        last,
                        e1_stop,
                        e2_stop,
                        state.skipped_master_frames.load(Ordering::SeqCst),
                        state.dropped_frames_total.load(Ordering::SeqCst),
                    ));
                }
            }
            _ if scenario.is_some() && after.is_none() => {
                after = Some((
                    r.master_frame,
                    state.skipped_master_frames.load(Ordering::SeqCst),
                    state.dropped_frames_total.load(Ordering::SeqCst),
                ));
            }
            _ => {}
        }
        after.is_none() && reports.len() < 60
    })
    .await;

    let (last, e1_stop, e2_stop, skipped0, dropped0) =
        scenario.expect("the loop drew on E1 within its first 60 ticks");
    let (frame, skipped1, dropped1) = after.expect("a tick after the scenario");
    eprintln!(
        "run edges: drawn {last} on E1; E1 stopped in {e1_stop}, E2 in {e2_stop}; \
         the next tick read {frame:?}; skips {skipped0} -> {skipped1}, drops {dropped0} -> {dropped1}"
    );
    assert_eq!(frame, None, "precondition: the next tick woke into STOPPED");
    assert_eq!(
        (skipped1 - skipped0, dropped1 - dropped0),
        (0, 0),
        "a stop from another run is no edge of the run last drawn: frames {}..={} span E1 and E2 and must not be counted",
        last + 1,
        e2_stop - 1
    );
}
