//! The frame watchdog (SPEC §10.3): a deadline accumulator with a fault
//! counter, logging, the fallback-slate trigger, and — since Prompt 11 WU8 —
//! the recovery that takes its slate down again.

use crate::state::{EngineState, FallbackSource};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// K: consecutive on-time View frames that clear a tripped watchdog's slate
/// (SPEC §10.3, v0.4.7; Prompt 11 WU8, a named choice).
///
/// Argued from the trip. The trip is an accumulation over one unbroken run:
/// `ceil(late / budget)` summed over consecutive late frames, reset by any
/// on-time frame, tripping above 2 — so as little as one frame late by more
/// than two budgets, and typically three late frames in a row. Recovery is the
/// same accumulation read the other way: an unbroken run of on-time frames,
/// reset by any late one.
///
/// Why 30 and not 3. Hysteresis: if clearing took as little evidence as
/// tripping, a borderline machine — late frames arriving in short runs — would
/// alternate slate and content at frame rate. At 30 the fastest possible cycle
/// is 1 frame to trip and 30 to clear, so the slate cannot change state faster
/// than about once a second at 30 fps, and one on-time frame in a bad patch
/// clears nothing. And 30 is the tree's own measure of "the pressure has
/// cleared": the ladder restores to nominal after `RESTORE_AFTER_ON_TIME` = 30
/// consecutive on-time frames (`render.rs`), so the slate comes down on the
/// same frame the ladder stands down, never while the ladder still reads the
/// machine as under pressure.
pub const WATCHDOG_CLEAR_AFTER_ON_TIME: u64 = 30;

/// Accumulates missed frames; above `threshold` the View is in trouble and the
/// watchdog's slate goes live, until `WATCHDOG_CLEAR_AFTER_ON_TIME`
/// consecutive on-time frames take it down.
pub struct Watchdog {
    state: Arc<EngineState>,
    threshold: u64,
    consecutive_misses: u64,
    /// The watchdog holds the slate (between a trip and its recovery).
    tripped: bool,
    /// On-time frames in a row since the last late one, while tripped.
    consecutive_on_time: u64,
}

impl Watchdog {
    /// `threshold` is the accumulated missed frames tolerated before the
    /// watchdog trips (the render loop uses 2: see `WATCHDOG_CLEAR_AFTER_ON_TIME`
    /// and SPEC §10.3).
    pub fn new(state: Arc<EngineState>, threshold: u64) -> Self {
        Self {
            state,
            threshold,
            consecutive_misses: 0,
            tripped: false,
            consecutive_on_time: 0,
        }
    }

    /// One frame's deadline report. `frames_missed == 0` means on time.
    pub fn report_frame(&mut self, frames_missed: u64) {
        if frames_missed == 0 {
            self.consecutive_misses = 0;
            if self.tripped {
                self.consecutive_on_time += 1;
                if self.consecutive_on_time >= WATCHDOG_CLEAR_AFTER_ON_TIME {
                    self.tripped = false;
                    self.consecutive_on_time = 0;
                    self.state.release_fallback(FallbackSource::Watchdog);
                    self.state
                        .watchdog_clears_total
                        .fetch_add(1, Ordering::SeqCst);
                    tracing::info!(
                        on_time = WATCHDOG_CLEAR_AFTER_ON_TIME,
                        "watchdog: recovered; clearing its fallback slate"
                    );
                }
            }
            return;
        }
        self.consecutive_on_time = 0;
        self.consecutive_misses += frames_missed;
        if self.consecutive_misses > self.threshold && !self.tripped {
            // One trip per episode: counted and logged once, not on every
            // late frame that follows while the slate is already up.
            self.tripped = true;
            self.state.engage_fallback(FallbackSource::Watchdog);
            self.state
                .watchdog_trips_total
                .fetch_add(1, Ordering::SeqCst);
            tracing::error!(
                misses = self.consecutive_misses,
                "watchdog: fault — threshold crossed; activating fallback slate"
            );
        }
    }

    /// The watchdog holds the slate.
    pub fn fault_active(&self) -> bool {
        self.tripped
    }
}

/// Watchdog cadence; Prompt 03 poller (frames arrive in 04).
pub const TICK: Duration = Duration::from_millis(33);
