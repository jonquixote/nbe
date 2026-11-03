//! The null sink renders the blocks a late wake missed (the user's rulings of 2026-11-02, late,
//! and 2026-11-03).
//!
//! The cadence used to resynchronize on a late wake, so the missed blocks were never rendered and
//! the record and stream taps, which carry what renders, lost that audio time with no line. CI's
//! dress telemetry read it on every run: the null sink rendered 27-47% of wall time, and the dress's
//! step 9 (AAC messages in 4 s) and step 11 (the take's duration) failed on macos-15, the first image
//! where they run.
//!
//! The guard is deterministic. Each test injects a late wake on a clock it controls (an instant
//! handed to `render_due`, a scripted `Pace` for the production loop, a paused tokio runtime for
//! the tests' loop) and asserts the blocks rendered equal the blocks due. No count depends on how
//! fast the machine runs.
//!
//! One more test holds the tests' loop live while it catches up: behind its schedule it no longer
//! sleeps, and before it yielded there, a sink slower than real time starved a current-thread
//! runtime for good.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::audio_driver::{
    render_due, run, run_blocking_paced, AudioDriver, AudioSink, NullSink, Pace,
    MAX_CATCH_UP_BLOCKS,
};
use nbe_engine::state::EngineState;

const HOUSE_RATE: u32 = 30;
const BLOCK: usize = 48_000 / HOUSE_RATE as usize;
const NS: Duration = Duration::from_nanos(1);

fn running_state() -> Arc<EngineState> {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    state.clock.lock().unwrap().start();
    state
}

fn blocks(driver: &AudioDriver) -> u64 {
    driver.graph.rendered_samples() / BLOCK as u64
}

/// A wake `late` past the deadline `next`, on a fresh driver: the blocks rendered, the schedule's
/// next deadline after, and the underruns counted.
fn wake(late: Duration) -> (u64, Duration, u64) {
    let state = running_state();
    let mut driver = AudioDriver::new(state.clone(), Box::new(NullSink::new(BLOCK)), HOUSE_RATE);
    let block = driver.block_duration();
    let origin = Instant::now();
    let mut next = origin;
    render_due(&mut driver, &state, &mut next, block, origin + late);
    (blocks(&driver), next - origin, driver.graph.underruns())
}

#[test]
fn a_late_wake_renders_every_block_due() {
    let block = AudioDriver::new(running_state(), Box::new(NullSink::new(BLOCK)), HOUSE_RATE)
        .block_duration();
    // (how late, blocks due): the block at `next`, and every later deadline behind the wake. A
    // deadline the wake lands on exactly is left to the next pass.
    let cases = [
        (Duration::ZERO, 1),
        (block / 2, 1),
        (block, 1),
        (block + NS, 2),
        (block * 5 / 2, 3),
        (block * 5 + NS, 6),
        (block * MAX_CATCH_UP_BLOCKS + NS, MAX_CATCH_UP_BLOCKS + 1),
    ];
    for (late, due) in cases {
        let (rendered, next, underruns) = wake(late);
        assert_eq!(
            rendered,
            u64::from(due),
            "a wake {late:?} late: {due} blocks due, {rendered} rendered (the resync rendered 1)"
        );
        assert_eq!(
            next,
            block * due,
            "a wake {late:?} late: the schedule keeps its phase, one deadline per block rendered"
        );
        assert_eq!(
            underruns,
            u64::from(due > 1),
            "a wake {late:?} late: a late wake is one underrun (SPEC §8.10), an on-time one none"
        );
    }
}

#[test]
fn a_wake_past_the_cap_renders_the_cap_and_gives_the_rest_up() {
    let block = AudioDriver::new(running_state(), Box::new(NullSink::new(BLOCK)), HOUSE_RATE)
        .block_duration();
    let cap = MAX_CATCH_UP_BLOCKS + 1;
    // (how late, blocks due): one past the cap, and a hundred blocks.
    for (late, due) in [(block * cap + NS, cap + 1), (block * 201 / 2, 101)] {
        let (rendered, next, underruns) = wake(late);
        assert_eq!(
            rendered,
            u64::from(cap),
            "a wake {late:?} late: the block it woke for and {MAX_CATCH_UP_BLOCKS} caught up, no more"
        );
        assert_eq!(
            next,
            block * due,
            "a wake {late:?} late: the rest is given up, and the schedule keeps its phase"
        );
        assert_eq!(underruns, 1, "a wake {late:?} late: one underrun");
    }
}

/// A sink that counts the blocks it takes.
struct Counting(Arc<AtomicU64>);

impl AudioSink for Counting {
    fn write(&mut self, _block: &[f32]) -> bool {
        self.0.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn block_frames(&self) -> usize {
        BLOCK
    }
}

/// A clock that moves only when the loop sleeps. Every sleep lands on its deadline exactly, except
/// sleep `late_at`, which lands `late_by` past it. The loop stops after sleep `stop_after`.
struct Scripted<'a> {
    now: Instant,
    sleeps: u32,
    late_at: u32,
    late_by: Duration,
    stop_after: u32,
    stop: &'a AtomicBool,
}

impl Pace for Scripted<'_> {
    fn now(&mut self) -> Instant {
        self.now
    }
    fn sleep(&mut self, d: Duration) {
        self.sleeps += 1;
        self.now += d;
        if self.sleeps == self.late_at {
            self.now += self.late_by;
        }
        if self.sleeps == self.stop_after {
            self.stop.store(true, Ordering::SeqCst);
        }
    }
}

/// The blocks due from `origin` to `end`: the deadline at `origin` and every one up to `end`.
fn due_between(origin: Instant, end: Instant, block: Duration) -> u64 {
    ((end - origin).as_nanos() / block.as_nanos()) as u64 + 1
}

#[test]
fn the_production_loop_renders_every_block_due_through_a_late_wake() {
    let state = running_state();
    let writes = Arc::new(AtomicU64::new(0));
    let driver = AudioDriver::new(
        state.clone(),
        Box::new(Counting(writes.clone())),
        HOUSE_RATE,
    );
    let block = driver.block_duration();
    let stop = AtomicBool::new(false);
    let origin = Instant::now();
    let mut pace = Scripted {
        now: origin,
        sleeps: 0,
        late_at: 5,
        late_by: block * 11 / 2,
        stop_after: 20,
        stop: &stop,
    };
    run_blocking_paced(driver, state, &stop, &mut pace);
    let due = due_between(origin, pace.now, block);
    assert_eq!(
        writes.load(Ordering::SeqCst),
        due,
        "the sink took every block due through a wake 5.5 blocks late ({due}); the resync lost 5"
    );
}

#[tokio::test(start_paused = true)]
async fn the_test_loop_renders_every_block_due_through_a_late_wake() {
    let state = running_state();
    let writes = Arc::new(AtomicU64::new(0));
    let driver = AudioDriver::new(
        state.clone(),
        Box::new(Counting(writes.clone())),
        HOUSE_RATE,
    );
    let block = driver.block_duration();
    let origin = tokio::time::Instant::now();
    let handle = tokio::spawn(run(driver, state));
    // On time to the middle of block 5: the paused clock steps from deadline to deadline.
    tokio::time::sleep(block * 11 / 2).await;
    // The late wake: the loop is asleep toward deadline 6, and the clock jumps 5 blocks past it.
    tokio::time::advance(block * 5).await;
    // On time again to the middle of block 20.
    tokio::time::sleep_until(origin + block * 41 / 2).await;
    let taken = writes.load(Ordering::SeqCst);
    handle.abort();
    let due = due_between(
        origin.into_std(),
        tokio::time::Instant::now().into_std(),
        block,
    );
    assert_eq!(
        taken, due,
        "the sink took every block due through a wake 4.5 blocks late ({due}); the resync lost 4"
    );
}

/// A sink slower than real time: each 1 ms block takes 2 ms, so the loop is always behind. It
/// counts the blocks it takes, and the blocks since the test's task last ran; past `limit` of
/// those it panics, which ends the loop's task so a starved test fails instead of hanging.
struct Slow {
    taken: Arc<AtomicU64>,
    since_test_ran: Arc<AtomicU64>,
    limit: u64,
}

impl AudioSink for Slow {
    fn write(&mut self, _block: &[f32]) -> bool {
        std::thread::sleep(Duration::from_millis(2));
        self.taken.fetch_add(1, Ordering::SeqCst);
        let since = self.since_test_ran.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(
            since <= self.limit,
            "the test's task has not run for {since} blocks"
        );
        true
    }
    fn block_frames(&self) -> usize {
        48
    }
}

// A current-thread runtime, as `#[tokio::test]` gives prompt06's
// `falling_behind_the_cadence_is_an_underrun`, which hung on this when the loop stopped sleeping
// once behind. The sink's slowness is wall time, but nothing asserted is: the count holds on any
// machine, because a slow sink only keeps the loop behind.
#[tokio::test]
async fn the_test_loop_gives_the_runtime_back_while_it_catches_up() {
    let pass = u64::from(MAX_CATCH_UP_BLOCKS + 1);
    let state = running_state();
    let taken = Arc::new(AtomicU64::new(0));
    let since_test_ran = Arc::new(AtomicU64::new(0));
    let driver = AudioDriver::new(
        state.clone(),
        Box::new(Slow {
            taken: taken.clone(),
            since_test_ran: since_test_ran.clone(),
            // One pass renders at most `pass` blocks; two allowed, so the guard does not lean on
            // the order the scheduler runs ready tasks in.
            limit: 2 * pass,
        }),
        HOUSE_RATE,
    );
    let handle = tokio::spawn(run(driver, state));
    let mut most = 0;
    while taken.load(Ordering::SeqCst) < 6 * pass && !handle.is_finished() {
        most = most.max(since_test_ran.swap(0, Ordering::SeqCst));
        tokio::task::yield_now().await;
    }
    most = most.max(since_test_ran.swap(0, Ordering::SeqCst));
    let finished = handle.is_finished();
    handle.abort();
    assert!(
        !finished && most <= 2 * pass,
        "a loop behind its schedule rendered {most} blocks without giving the runtime back"
    );
}
