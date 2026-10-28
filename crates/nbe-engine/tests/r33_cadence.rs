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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loop_asked_to_tick_faster_than_the_clock_draws_each_master_frame_once() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    state.clock.lock().unwrap().start();
    let mut reports = Vec::new();
    // A 60 Hz loop against the 30 fps clock: a loop that ticks on its own
    // grid reads every master frame twice.
    run_loop(&mut render, &state, RATE * 2, |r| {
        reports.push(*r);
        reports.len() < 90
    })
    .await;
    let (frames, repeats, skips) = read(&reports);
    eprintln!(
        "r33 faster loop: {} ticks, repeats {repeats}, skips {skips}",
        frames.len()
    );
    assert_eq!(repeats, 0, "a master frame was drawn twice: {frames:?}");
    assert!(
        frames.windows(2).all(|w| w[1] > w[0]),
        "out of order: {frames:?}"
    );
    assert_eq!(state.repeated_master_frames.load(Ordering::SeqCst), 0);
    assert_eq!(state.skipped_master_frames.load(Ordering::SeqCst), skips);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loop_started_just_ahead_of_a_clock_boundary_never_draws_a_master_frame_twice() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    // R33's phase: the loop's start sits `lead` before a clock boundary. A
    // loop that reads the clock wherever it wakes then reads the next frame on
    // any wake later than `lead`, and the same frame on an earlier one, so it
    // repeats where `lead` sits inside the timer's lateness. That lateness
    // differs by machine and OS (about 2 ms in a debug test on the reference
    // machine, 2026-10-28), so the leads sweep it.
    let mut all = Vec::new();
    for lead_us in [500, 1_000, 1_500, 2_000, 2_500, 3_000, 4_000] {
        state.clock.lock().unwrap().stop();
        start_clock_phased(&state, Duration::from_micros(lead_us));
        let mut reports = Vec::new();
        run_loop(&mut render, &state, RATE, |r| {
            reports.push(*r);
            reports.len() < 45
        })
        .await;
        let (frames, repeats, skips) = read(&reports);
        let mut late: Vec<u128> = reports.iter().map(|r| r.woke_late.as_micros()).collect();
        late.sort_unstable();
        eprintln!(
            "r33 phased, lead {lead_us} µs: repeats {repeats}, skips {skips}; woke late p50 {} µs, max {} µs",
            late[late.len() / 2],
            late[late.len() - 1]
        );
        assert_eq!(
            repeats, 0,
            "lead {lead_us} µs: a master frame was drawn twice: {frames:?}"
        );
        assert!(
            frames.windows(2).all(|w| w[1] > w[0]),
            "lead {lead_us} µs: out of order: {frames:?}"
        );
        all.push(skips);
    }
    assert_eq!(state.repeated_master_frames.load(Ordering::SeqCst), 0);
    assert_eq!(
        state.skipped_master_frames.load(Ordering::SeqCst),
        all.iter().sum::<u64>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_stalled_tick_the_loop_draws_the_clock_s_frame_and_never_bursts() {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    state.clock.lock().unwrap().start();
    let mut reports = Vec::new();
    let mut at = Vec::new();
    run_loop(&mut render, &state, RATE, |r| {
        reports.push(*r);
        at.push(Instant::now());
        if reports.len() == 30 {
            std::thread::sleep(budget() * 7 / 2);
        }
        reports.len() < 60
    })
    .await;
    let (frames, repeats, skips) = read(&reports);
    let before = frames[29];
    let after = frames[30];
    // Stalled 3.5 frames past tick 30's draw: the next tick draws the frame
    // the clock names then, three or four on, never the one after `before`.
    assert!(
        after >= before + 3,
        "the loop caught up instead of drawing the clock's frame: {frames:?}"
    );
    assert_eq!(repeats, 0, "{frames:?}");
    assert_eq!(state.skipped_master_frames.load(Ordering::SeqCst), skips);
    assert!(skips >= 2, "{frames:?}");
    // No burst: the ten ticks after the stall are each a frame apart, give or
    // take the timer, never drawn back to back to make the frames up.
    let gaps: Vec<Duration> = at[31..41].windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        gaps.iter().all(|g| *g >= budget() / 2),
        "ticks ran back to back after the stall: {gaps:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_skip_is_logged_with_the_master_frames_never_drawn() {
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
    let mut reports = Vec::new();
    run_loop(&mut render, &state, RATE, |r| {
        reports.push(*r);
        if reports.len() == 10 {
            std::thread::sleep(budget() * 7 / 2);
        }
        reports.len() < 20
    })
    .await;
    let (frames, _, _) = read(&reports);
    let gap = frames
        .windows(2)
        .find(|w| w[1] > w[0] + 1)
        .unwrap_or_else(|| panic!("the stall skipped nothing: {frames:?}"));
    let expected = format!(
        "R33: master frames {}..={} were never drawn",
        gap[0] + 1,
        gap[1] - 1
    );
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains(&expected), "no line `{expected}` in:\n{log}");
}
