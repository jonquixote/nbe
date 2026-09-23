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

// ---------------------------------------------------------------------------
// The loop's two seams (ZERO-COPY Phase 3b, step 5)
// ---------------------------------------------------------------------------

/// What [`begin_tap_frame`] decided, carried across the draw.
///
/// `surface` is the pool's loan for THIS frame; holding it is what keeps the
/// surface out of the free list until the encoder is done.
#[derive(Debug, Default)]
pub struct TapLoan {
    surface: Option<Arc<SharedSurface>>,
    /// True when the take is on the zero-copy path at all. Distinguishes "this
    /// take is CPU readback" (`false`, `surface` None) from "this take is
    /// zero-copy but the pool was empty" (`true`, `surface` None) — the second
    /// has already counted its skip and must not count a second one, nor fall
    /// through to a readback the take did not choose.
    zero_copy_take: bool,
}

impl TapLoan {
    pub fn is_zero_copy(&self) -> bool {
        self.zero_copy_take
    }
    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }
    /// Read-only share of this frame's surface (G1 both-live: one composite,
    /// N `Arc` holders). Cloning never moves the loan — the record handoff
    /// below is unchanged. No behavior change: pure getter.
    pub fn surface(&self) -> Option<Arc<SharedSurface>> {
        self.surface.clone()
    }
}

/// **Before the draw**: take this frame's surface and point the View at it.
///
/// The free-surface question cannot wait for the budget pre-check, which needs
/// the View's measured time and therefore runs after `render_frame`. On the
/// zero-copy path the draw goes INTO the surface, so by then a missing surface
/// has already cost a corrupted frame rather than a skipped one.
///
/// **Chain loss is an error here, not a quiet CPU frame.** A take that claims
/// `zeroCopy` and finds no pool has lost the chain mid-take, and Option A
/// (`docs/zero-copy-p3-design.md`, Q3') says such a take ends loudly rather
/// than substituting a transport nobody chose. `claims_zero_copy` is what
/// separates that from an ordinary CPU take, which also has no pool.
pub fn begin_tap_frame(
    render: &mut crate::render::RenderLoop,
    pool: Option<&SurfacePool>,
    claims_zero_copy: bool,
    skipped: &AtomicU64,
) -> Result<TapLoan, anyhow::Error> {
    let Some(pool) = pool else {
        if claims_zero_copy {
            anyhow::bail!(
                "E_NO_ZEROCOPY: the take's surface pool is gone mid-take; \
                 the chain was available at record.start and is not now"
            );
        }
        return Ok(TapLoan::default());
    };
    let surface = acquire_record_surface(pool, skipped);
    if let Some(s) = &surface {
        render.set_view_surface(Some(s.clone()))?;
    }
    Ok(TapLoan {
        surface,
        zero_copy_take: true,
    })
}

/// **Immediately after the draw**: put the View back on the built-in target.
///
/// Separate from [`end_tap_frame`] for two reasons. It must run whether or not
/// the take has a handoff endpoint — a retarget left in place would composite
/// the NEXT frame into a surface nobody is holding — and it needs `&mut
/// RenderLoop`, which cannot coexist with the readback closure `end_tap_frame`
/// takes (`&render`).
pub fn restore_view(render: &mut crate::render::RenderLoop, loan: &TapLoan) {
    if loan.zero_copy_take {
        // Infallible: clearing the retarget has no geometry to check.
        let _ = render.set_view_surface(None);
    }
}

/// **After the draw**: skip, or hand the frame off.
///
/// Returns the frame's `record_tap_ms` contribution. `readback` is the caller's
/// View readback, needed only on the CPU path and NOT awaited on the zero-copy
/// path — that removal is what the whole migration is for.
pub async fn end_tap_frame<F, Fut>(
    loan: TapLoan,
    render_elapsed: Duration,
    budget: Option<Duration>,
    tx: &SyncSender<RecordMsg>,
    skipped: &AtomicU64,
    readback: F,
) -> f64
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = (Vec<u8>, Duration)>,
{
    // A zero-copy take that found no free surface already counted its skip,
    // before the draw. Counting again here would double-count, and falling
    // through to a readback would run the path this take did not choose.
    if loan.zero_copy_take && loan.surface.is_none() {
        return 0.0;
    }
    if should_skip_record_frame(render_elapsed, budget) {
        skipped.fetch_add(1, Ordering::SeqCst);
        return 0.0;
    }
    let outcome = match loan.surface {
        Some(surface) => handoff_record_surface(surface, tx),
        None => {
            let (rgba, elapsed) = readback().await;
            handoff_record_frame(rgba, tx, elapsed)
        }
    };
    if !outcome.sent {
        skipped.fetch_add(1, Ordering::SeqCst);
    }
    outcome.feed_ms
}

/// Mid-take chain loss: end the take, loudly (ZERO-COPY Phase 3b, step 6).
///
/// **New behaviour, named as such.** Nothing in the tree answered this before,
/// because no live take used the chain. The honest framing: the probe was right
/// at `record.start`, and the chain died mid-take — device loss, surface
/// invalidation.
///
/// This is Option A of the memo's Q3'. Option B — fall back to readback
/// mid-recording — keeps the file whole but was rejected on two counts from the
/// tree rather than from taste. `record_tap_path` is *per take* by
/// construction, written once at selection and read by every tick after, so
/// Option B makes the field a lie for part of every take it applies to — and
/// the field's entire purpose is that a fallback is visible. And the
/// loud-failure precedent is strong and recent: `record.stop`'s finalize
/// failure withholds the ack rather than reporting a success it cannot vouch
/// for. A take that silently changes its own transport is the same class of
/// quiet substitution that rule refuses.
///
/// What happens: the session is ABANDONED, not finished — the file is kept
/// exactly as it is, which §9.3's finalization-free fragment policy makes
/// playable — the state returns to Idle, and the take's `record_tap_selection`
/// is left alone, because `zeroCopy` is the truth about the take that was.
///
/// Option B becomes attractive the day `record_tap_path` can carry a transition
/// rather than a value. That is a wire change and belongs to whoever wants it.
pub fn end_take_on_chain_loss(state: &crate::state::EngineState, cause: &str) -> String {
    *state.record_tap.lock().unwrap() = None;
    if let Some(mut session) = state.record_session.lock().unwrap().take() {
        session.abandon();
    }
    *state.record_state.lock().unwrap() = crate::state::RecordState::Idle;
    let token = match cause.contains("E_NO_ZEROCOPY") {
        true => cause.to_string(),
        false => format!("E_NO_ZEROCOPY: {cause}"),
    };
    tracing::error!(
        err = %token,
        "record tap: zero-copy chain lost mid-take; take ended, file kept as-is"
    );
    token
}
