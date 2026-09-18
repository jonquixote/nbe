//! Main-loop record handoff (Prompt 09 WU-pipe, SPEC §9.3).
//!
//! Runs AFTER `render_frame` and its deadline check, which are untouched. Two
//! pure functions the loop calls per recorded frame:
//!
//! 1. [`should_skip_record_frame`]: the budget pre-check. When the View's
//!    measured time already meets or exceeds the frame budget, the record
//!    frame SKIPS before any readback — no readback await, no encode, no
//!    pushes. Un-timed: a skip costs nothing by definition. Record degrades,
//!    View never waits.
//! 2. [`handoff_record_frame`]: `try_send` the readback to the record thread
//!    over the bounded channel. A full channel sheds (never blocks) and
//!    reports `sent=false` so the loop counts the skip. The caller's measured
//!    readback plus the handoff itself form [`HandoffOutcome::feed_ms`] — the
//!    loop's `record_tap_ms` input, kept OFF the render budget by construction
//!    (measured here, only ever added to the record counter).
//!
//! No threads, no file I/O, no encode here: the record thread
//! ([`thread`](crate::record::thread)) owns the encoder, the writer, and the
//! tap drain. `record.stop` / `show.stop` quiescence ends the take there.

use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use crate::record::thread::RecordMsg;

/// What one record handoff did, for the loop's separate accounting and tests.
#[derive(Debug)]
pub struct HandoffOutcome {
    /// True when the frame reached the record thread's queue.
    pub sent: bool,
    /// Handoff cost in milliseconds: the caller's measured readback PLUS the
    /// `try_send` — the loop's `record_tap_ms` input, kept OFF the render
    /// budget by construction. Zero on a shed handoff performed with
    /// `Duration::ZERO` readback (nothing was spent: the frame never left).
    pub feed_ms: f64,
}

/// Budget pre-check (degradation ladder: record yields, View never waits).
/// `render_elapsed` is the View's measured time this frame, `budget` its
/// frame budget (`None` while STOPPED: always feed — a missed deadline is
/// meaningless with no show clock running).
pub fn should_skip_record_frame(render_elapsed: Duration, budget: Option<Duration>) -> bool {
    match budget {
        Some(b) => render_elapsed >= b,
        None => false,
    }
}

/// Hand one recorded frame to the record thread. `rgba` is the View readback
/// the caller already awaited; `readback_elapsed` is how long that await cost
/// (folded into `feed_ms`, never into the render budget). Never blocks: a
/// full (or gone) channel sheds and reports `sent=false` for the loop's skip
/// counter.
pub fn handoff_record_frame(
    rgba: Vec<u8>,
    tx: &SyncSender<RecordMsg>,
    readback_elapsed: Duration,
) -> HandoffOutcome {
    let readback_ms = readback_elapsed.as_secs_f64() * 1000.0;
    let started = Instant::now();
    match tx.try_send(RecordMsg::Frame { rgba }) {
        Ok(()) => HandoffOutcome {
            sent: true,
            feed_ms: readback_ms + started.elapsed().as_secs_f64() * 1000.0,
        },
        Err(_) => HandoffOutcome {
            sent: false,
            feed_ms: readback_ms,
        },
    }
}
