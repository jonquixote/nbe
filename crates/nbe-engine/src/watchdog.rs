//! Frame watchdog skeleton (Step 6, SPEC 10.3): a deadline checker with a
//! fault counter, logging, and the fallback-slate trigger path. With no
//! frames in flight yet, this validates the mechanism, not the pixels.

use crate::state::EngineState;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Counts consecutive deadline misses; when it exceeds `threshold` the view
/// is in trouble and the fallback slate goes live.
pub struct Watchdog {
    state: Arc<EngineState>,
    threshold: u64,
    consecutive_misses: u64,
}

impl Watchdog {
    /// `threshold` is the number of consecutive misses tolerated before the
    /// fault affects the fallback trigger (SPEC 10.3: > 1 frame missed).
    pub fn new(state: Arc<EngineState>, threshold: u64) -> Self {
        Self {
            state,
            threshold,
            consecutive_misses: 0,
        }
    }

    /// One frame's deadline report. frames_missed == 0 means on time.
    pub fn report_frame(&mut self, frames_missed: u64) {
        if frames_missed == 0 {
            self.consecutive_misses = 0;
            return;
        }
        self.consecutive_misses += frames_missed;
        if self.consecutive_misses > self.threshold {
            tracing::error!(
                misses = self.consecutive_misses,
                "watchdog: threshold crossed; activating fallback slate"
            );
            self.state.fallback_active.store(true, Ordering::SeqCst);
        }
    }

    pub fn fault_active(&self) -> bool {
        self.consecutive_misses > self.threshold
    }
}

/// Watchdog cadence; Prompt 03 poller (frames arrive in 04).
pub const TICK: Duration = Duration::from_millis(33);
