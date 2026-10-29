//! R33's run edges (#77's precision note 1): a skip was counted only between
//! two running ticks, so the master frames at a run's edges went uncounted —
//! a stall across the clock's start, or a wake into STOPPED. Each such frame
//! is a master frame never drawn, so a dropped frame (§10.2, ruled
//! 2026-10-28). The production loop (`tick::run_loop`) on a real
//! `RenderLoop`, as in `r33_cadence.rs`; the stall is the loop's own thread
//! held inside `on_tick`, where a directive would apply.

use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
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

/// Log lines captured from the loop's thread.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture() -> (Captured, tracing::subscriber::DefaultGuard) {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (captured, guard)
}

fn log_of(captured: &Captured) -> String {
    String::from_utf8(captured.0.lock().unwrap().clone()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stall_across_the_clock_s_start_counts_the_frames_never_drawn() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    let (captured, _guard) = capture();
    let mut reports = Vec::new();
    run_loop(&mut render, &state, RATE, |r| {
        reports.push(*r);
        if reports.len() == 5 {
            // The clock starts, and the loop's thread is held two and a half
            // frames before its first running tick.
            state.clock.lock().unwrap().start();
            std::thread::sleep(budget() * 5 / 2);
        }
        reports.len() < 40
    })
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
    let log = log_of(&captured);
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
    let (captured, _guard) = capture();
    state.clock.lock().unwrap().start();
    let mut reports = Vec::new();
    run_loop(&mut render, &state, RATE, |r| {
        reports.push(*r);
        if reports.len() == 20 {
            // The loop's thread is held three and a half frames, then the
            // clock stops: the loop's next wake reads STOPPED.
            std::thread::sleep(budget() * 7 / 2);
            state.clock.lock().unwrap().stop();
        }
        reports.len() < 30
    })
    .await;

    let last = reports[19].master_frame.expect("tick 20 ran on the clock");
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
    let log = log_of(&captured);
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
        .find(|l| l.contains("cadence: run ended after 20 ticks"))
        .unwrap_or_else(|| panic!("{log}"));
    assert!(
        run_line.contains(&format!("skips {}", in_run + edge)),
        "{run_line}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_and_a_stop_with_no_stall_count_nothing_at_the_edges() {
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
    assert_eq!(first, 0, "the run's first tick draws frame 0");
    assert_eq!(
        state.skipped_master_frames.load(Ordering::SeqCst),
        in_run_skips(&reports),
        "a run with no stall at its edges counts nothing there"
    );
}
