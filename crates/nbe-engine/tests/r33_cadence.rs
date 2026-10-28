//! R33: the render loop's cadence against the master clock (SPEC §11.3). The
//! production loop (`tick::run_loop`) driven on a real `RenderLoop` (§2a rule
//! 7): the master frames it draws, the repeats and skips it counts, and the
//! summary a run and a record take end with.
//!
//! R33 (filed 2026-10-26): the loop slept to its own boundary and then read
//! the clock, so two ticks could read the same master frame — 630, 630, then
//! 632 — and nothing counted it.

use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, RecordState};
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

/// Start the master clock, then return `lead` before one of its frame
/// boundaries, so a loop started now has its own boundaries `lead` ahead of
/// the clock's: R33's phase, where a late wake reads the next frame.
fn start_clock_phased(state: &EngineState, lead: Duration) {
    state.clock.lock().unwrap().start();
    let epoch = state
        .clock
        .lock()
        .unwrap()
        .epoch()
        .expect("a running clock has an epoch");
    let target = epoch + budget() * 3 - lead;
    if let Some(coarse) = target.checked_duration_since(Instant::now()) {
        std::thread::sleep(coarse.saturating_sub(Duration::from_millis(2)));
    }
    while Instant::now() < target {
        std::hint::spin_loop();
    }
}

/// The master frames a run of reports drew, and the repeats and skips in
/// them, read from the reports alone.
fn read(reports: &[TickReport]) -> (Vec<u64>, u64, u64) {
    let frames: Vec<u64> = reports.iter().filter_map(|r| r.master_frame).collect();
    let repeats = frames.windows(2).filter(|w| w[1] == w[0]).count() as u64;
    let skips = frames
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0] + 1))
        .sum();
    (frames, repeats, skips)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_loop_counts_every_repeat_and_skip_it_draws() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    start_clock_phased(&state, Duration::from_micros(500));
    let mut reports = Vec::new();
    run_loop(&mut render, &state, RATE, |r| {
        reports.push(*r);
        if reports.len() == 40 {
            // One stalled tick: the loop wakes over two frames late.
            std::thread::sleep(budget() * 5 / 2);
        }
        reports.len() < 120
    })
    .await;
    let (frames, repeats, skips) = read(&reports);
    eprintln!(
        "r33 counted: {} ticks, repeats {repeats}, skips {skips}",
        frames.len()
    );
    assert_eq!(frames.len(), 120, "every tick ran with the clock RUNNING");
    assert!(
        skips >= 1,
        "the stalled tick skipped no master frame: {frames:?}"
    );
    assert_eq!(
        state.repeated_master_frames.load(Ordering::SeqCst),
        repeats,
        "the repeat counter disagrees with the frames drawn: {frames:?}"
    );
    assert_eq!(
        state.skipped_master_frames.load(Ordering::SeqCst),
        skips,
        "the skip counter disagrees with the frames drawn: {frames:?}"
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_take_and_a_run_each_end_with_one_summary_line() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    state.clock.lock().unwrap().start();
    let mut ticks = 0;
    run_loop(&mut render, &state, RATE, |_| {
        ticks += 1;
        match ticks {
            // A session-less take (the test seam): ticks 4 through 12 record.
            3 => *state.record_state.lock().unwrap() = RecordState::Recording,
            12 => *state.record_state.lock().unwrap() = RecordState::Idle,
            // The run: ticks 1 through 20 have the clock RUNNING.
            20 => state.clock.lock().unwrap().stop(),
            _ => {}
        }
        ticks < 25
    })
    .await;

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let summaries: Vec<&str> = log.lines().filter(|l| l.contains("cadence: ")).collect();
    assert_eq!(
        summaries.len(),
        2,
        "one line per take and per run: {summaries:#?}"
    );
    assert!(
        summaries[0].contains("cadence: record take ended after 9 ticks: repeats "),
        "{summaries:#?}"
    );
    assert!(
        summaries[1].contains("cadence: run ended after 20 ticks: repeats "),
        "{summaries:#?}"
    );
    for line in summaries {
        assert!(
            line.contains(", skips ")
                && line.contains(", drops ")
                && line.contains("woke late mean "),
            "{line}"
        );
    }
}
