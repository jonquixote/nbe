//! Why an output skipped a frame: each skip counted once, under the one path
//! that gave the frame up (R46's accounting, the user's ruling of 2026-11-04).
//!
//! `skipped_record_frames` and `skipped_stream_frames` each total skips from
//! several places: the render loop before and after the draw, and the
//! output's own thread. A total cannot say which, and on `macos-15`
//! `both_live`'s record and stream skips were neither over-budget draws nor an
//! exhausted pool (#84's CI: record skips 7, over-budget draws 0, stream
//! skips 7, the stream's own pool untouched). [`SkipCount`] keeps the total
//! the engine has always kept and, beside it, one count per [`SkipPath`].
//! [`CountSkip::count`] moves the path and the total in one call, so a skip
//! is attributed to exactly one path. They are two atomics, the path added
//! first, so a reader between the two adds sees the paths ahead of the total:
//! the paths sum to the total whenever nothing is counting, which is why
//! `both_live` reads after both outputs have stopped. A test that asserts the
//! sum then checks the counter's own arithmetic. Each site's path is pinned by
//! the test that drives it, where a deterministic one exists: seven of the
//! sixteen paths (record and stream together) have one, and nine do not
//! (#84's body lists both).

use std::sync::atomic::{AtomicU64, Ordering};

/// The one place a skipped frame was given up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipPath {
    /// The loop, before the draw: no free surface. Record: the take's pool
    /// was exhausted. Stream: its own pool was busy, or the View refused the
    /// retarget to it.
    NoSurface,
    /// The loop, after the draw: the View's draw took the frame's whole
    /// budget, so record yields (the degradation ladder).
    OverBudget,
    /// The loop, after the draw: no draw to wait on (R31, R35). The View did
    /// not submit this frame, so no encoder may read the surface.
    NoDraw,
    /// The loop's handoff: the output's queue was full (record: or gone).
    QueueFull,
    /// The output's thread: no encoder is open.
    NoEncoder,
    /// The output's thread: the View's draw did not finish within
    /// [`DRAW_WAIT`](crate::record::DRAW_WAIT).
    DrawWait,
    /// The output's thread: the encoder refused the frame.
    Refused,
    /// The stream thread: a frame older than the stream's first, with no
    /// timestamp to take.
    BeforeFirst,
    /// The stream thread: the first units had no sequence header to announce.
    NoSequenceHeader,
    /// The stream thread: the publisher shed a unit (counted per unit).
    Shed,
}

impl SkipPath {
    pub const ALL: [SkipPath; 10] = [
        SkipPath::NoSurface,
        SkipPath::OverBudget,
        SkipPath::NoDraw,
        SkipPath::QueueFull,
        SkipPath::NoEncoder,
        SkipPath::DrawWait,
        SkipPath::Refused,
        SkipPath::BeforeFirst,
        SkipPath::NoSequenceHeader,
        SkipPath::Shed,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SkipPath::NoSurface => "no surface",
            SkipPath::OverBudget => "over budget",
            SkipPath::NoDraw => "no draw",
            SkipPath::QueueFull => "queue full",
            SkipPath::NoEncoder => "no encoder",
            SkipPath::DrawWait => "draw wait",
            SkipPath::Refused => "refused",
            SkipPath::BeforeFirst => "before first",
            SkipPath::NoSequenceHeader => "no sequence header",
            SkipPath::Shed => "shed",
        }
    }
}

/// Where a skip is counted.
pub trait CountSkip: Send + Sync + std::fmt::Debug {
    /// Count `n` frames given up at `path`.
    fn count(&self, path: SkipPath, n: u64);
}

/// A bare total, for a caller that keeps no paths (a test of one seam).
impl CountSkip for AtomicU64 {
    fn count(&self, _path: SkipPath, n: u64) {
        self.fetch_add(n, Ordering::SeqCst);
    }
}

/// One output's skipped frames: the total, and the count at each path.
#[derive(Debug, Default)]
pub struct SkipCount {
    total: AtomicU64,
    paths: [AtomicU64; SkipPath::ALL.len()],
}

impl SkipCount {
    /// The total, every path together.
    pub fn load(&self, order: Ordering) -> u64 {
        self.total.load(order)
    }

    /// The count at one path.
    pub fn path(&self, path: SkipPath) -> u64 {
        self.paths[path as usize].load(Ordering::SeqCst)
    }

    /// The paths' counts added up: the total, when every skip named its path
    /// and nothing is counting.
    pub fn paths_total(&self) -> u64 {
        SkipPath::ALL.iter().map(|&p| self.path(p)).sum()
    }

    /// Zero the total and every path (a new take or stream).
    pub fn reset(&self) {
        self.total.store(0, Ordering::SeqCst);
        for p in &self.paths {
            p.store(0, Ordering::SeqCst);
        }
    }

    /// The paths that counted, as `path n` pairs: `none` when none did.
    pub fn by_path(&self) -> String {
        let counted: Vec<String> = SkipPath::ALL
            .iter()
            .filter(|&&p| self.path(p) > 0)
            .map(|&p| format!("{} {}", p.name(), self.path(p)))
            .collect();
        match counted.is_empty() {
            true => "none".into(),
            false => counted.join(", "),
        }
    }
}

impl CountSkip for SkipCount {
    fn count(&self, path: SkipPath, n: u64) {
        self.paths[path as usize].fetch_add(n, Ordering::SeqCst);
        self.total.fetch_add(n, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_skip_counts_its_path_and_the_total_once() {
        let skips = SkipCount::default();
        skips.count(SkipPath::NoDraw, 1);
        skips.count(SkipPath::NoDraw, 1);
        skips.count(SkipPath::Shed, 3);
        assert_eq!(skips.load(Ordering::SeqCst), 5);
        assert_eq!(skips.path(SkipPath::NoDraw), 2);
        assert_eq!(skips.path(SkipPath::Shed), 3);
        for p in SkipPath::ALL {
            if !matches!(p, SkipPath::NoDraw | SkipPath::Shed) {
                assert_eq!(skips.path(p), 0, "{} counted nothing", p.name());
            }
        }
        assert_eq!(skips.paths_total(), skips.load(Ordering::SeqCst));
        assert_eq!(skips.by_path(), "no draw 2, shed 3");
    }

    #[test]
    fn a_reset_zeroes_the_total_and_every_path() {
        let skips = SkipCount::default();
        for p in SkipPath::ALL {
            skips.count(p, 1);
        }
        assert_eq!(skips.load(Ordering::SeqCst), SkipPath::ALL.len() as u64);
        skips.reset();
        assert_eq!(skips.load(Ordering::SeqCst), 0);
        assert_eq!(skips.paths_total(), 0);
        assert_eq!(skips.by_path(), "none");
    }

    #[test]
    fn every_path_has_its_own_slot_and_name() {
        for (i, p) in SkipPath::ALL.iter().enumerate() {
            assert_eq!(*p as usize, i, "ALL is in declaration order");
        }
        let names: std::collections::HashSet<_> = SkipPath::ALL.iter().map(|p| p.name()).collect();
        assert_eq!(names.len(), SkipPath::ALL.len());
    }
}
