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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_decode::zerocopy::{SharedSurface, SurfacePool};

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

// ---------------------------------------------------------------------------
// The zero-copy tap's two extra seams (ZERO-COPY Phase 3b, step 3)
// ---------------------------------------------------------------------------

/// Take this frame's surface, or count a skip — **asked before the draw**.
///
/// [`should_skip_record_frame`] above runs AFTER `render_frame`, because it
/// needs the View's measured time. The free-surface question cannot wait that
/// long: on the zero-copy path the draw goes *into* the surface, so by the time
/// the budget check runs the damage a missing surface would do is already done.
/// Asking here preserves the discipline's promise — record degrades, the View
/// never waits — at the only point where it can still be kept.
///
/// The counter is the same `skipped_record_frames` the budget skip and the
/// shed handoff feed, deliberately: `record_tap_ms` and the skip count are how
/// the two paths are compared in a soak, and a skip that counted differently
/// by path would make the span counters incomparable exactly when the
/// comparison matters.
pub fn acquire_record_surface(
    pool: &SurfacePool,
    skipped: &AtomicU64,
) -> Option<Arc<SharedSurface>> {
    match pool.acquire() {
        Some(s) => Some(s),
        None => {
            skipped.fetch_add(1, Ordering::SeqCst);
            None
        }
    }
}

/// Hand one drawn-into surface to the record thread. The mirror of
/// [`handoff_record_frame`], minus the readback that no longer happens.
///
/// `readback_elapsed` has no analogue here and there is no parameter for one:
/// the zero-copy path's `feed_ms` is the `try_send` alone, which is the
/// measurement the before/after table compares. A shed still reports
/// `sent=false` so the loop counts it — but note that a shed here is now the
/// *rare* case, because the free-surface question already refused the frames
/// the channel had no room for.
pub fn handoff_record_surface(
    surface: Arc<SharedSurface>,
    tx: &SyncSender<RecordMsg>,
) -> HandoffOutcome {
    let started = Instant::now();
    match tx.try_send(RecordMsg::Surface { surface }) {
        Ok(()) => HandoffOutcome {
            sent: true,
            feed_ms: started.elapsed().as_secs_f64() * 1000.0,
        },
        Err(_) => HandoffOutcome {
            sent: false,
            feed_ms: 0.0,
        },
    }
}
