//! Video assets in the engine (Prompt 05 Steps 4–6).
//!
//! Everything expensive happens here, at load/arm time: open the asset, decode
//! its frames, plan its cache residency. The render loop only ever asks
//! "which frame index am I showing?" and samples a texture.
//!
//! Decode sessions are a capped resource (SPEC §24): VideoToolbox limits how
//! many can be active, so the pool below refuses to exceed its cap rather than
//! letting the platform fail unpredictably mid-show.

use crate::decode::{DecodeError, DecodeSession, DecodedFrame};
use crate::loop_cache::{self, CacheBudget, CachePlan, LoopSpec};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// SPEC §24: cap simultaneous decode sessions and reuse them. Eight is a
/// conservative ceiling well under the platform limit.
pub const MAX_DECODE_SESSIONS: u32 = 8;

/// Frames preloaded for a plain clip at load time.
///
/// A clip is not a loop: it plays once and does not need to be resident. Arm
/// time owes the render loop a first frame (Prompt 05 Step 4), not the whole
/// asset — preloading a 30-second 1080p clip in full costs seconds of load
/// time and hundreds of MiB for frames that will be shown once, if ever.
/// Loops, which are re-shown every period, are cached to the budget instead.
pub const CLIP_PRELOAD_FRAMES: usize = 30;

/// Tracks how many decode sessions are open, so `decodeSessions` telemetry is
/// a measurement rather than a guess.
#[derive(Debug, Default)]
pub struct SessionPool {
    inner: Arc<PoolInner>,
    cap: u32,
}

#[derive(Debug, Default)]
struct PoolInner {
    active: AtomicU32,
    peak: AtomicU32,
    refused: AtomicU32,
    /// Bumped by every [`SessionPool::release_all`]. A lease born in an older
    /// epoch was already reclaimed; its `Drop` must not decrement again.
    epoch: AtomicU64,
}

/// A live lease on a decode session. Owned, not borrowed: the lease keeps the
/// pool's counters alive via `Arc`, so a [`VideoAsset`] can hold its session
/// for the show's duration without self-referential state.
///
/// Dropping the lease returns the slot — unless a [`SessionPool::release_all`]
/// has since reclaimed it, in which case the stale lease drops silently. The
/// epoch guard plus a saturating decrement mean a
/// live-lease-after-`release_all` can never underflow `active` below zero.
#[derive(Debug)]
pub struct SessionLease {
    pool: Arc<PoolInner>,
    epoch: u64,
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        if self.pool.epoch.load(Ordering::SeqCst) != self.epoch {
            return; // reclaimed by release_all; nothing to return
        }
        let _ = self
            .pool
            .active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1));
    }
}

impl SessionPool {
    pub fn with_cap(cap: u32) -> Self {
        Self {
            cap,
            ..Default::default()
        }
    }

    pub fn new() -> Self {
        Self::with_cap(MAX_DECODE_SESSIONS)
    }

    /// Take a session slot, or `None` when the cap is reached. A refusal is
    /// counted: an operator seeing decode failures needs to know whether the
    /// cap or the media was the cause.
    pub fn acquire(&self) -> Option<SessionLease> {
        loop {
            let current = self.inner.active.load(Ordering::SeqCst);
            if current >= self.cap {
                self.inner.refused.fetch_add(1, Ordering::SeqCst);
                return None;
            }
            if self
                .inner
                .active
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                self.inner.peak.fetch_max(current + 1, Ordering::SeqCst);
                return Some(SessionLease {
                    pool: Arc::clone(&self.inner),
                    epoch: self.inner.epoch.load(Ordering::SeqCst),
                });
            }
        }
    }

    pub fn active(&self) -> u32 {
        self.inner.active.load(Ordering::SeqCst)
    }

    pub fn peak(&self) -> u32 {
        self.inner.peak.load(Ordering::SeqCst)
    }

    pub fn refused(&self) -> u32 {
        self.inner.refused.load(Ordering::SeqCst)
    }

    pub fn cap(&self) -> u32 {
        self.cap
    }

    /// Release all held decode sessions ([RI-8] unload-at-next-load).
    ///
    /// `show.stop` and the top of `show.load` (release-then-rebuild) call
    /// this: `active` drops to zero so `decodeSessions` telemetry (which
    /// reads `active`) clears within the grace window, while package
    /// residency (video rings, image textures, audio assets) is retained
    /// elsewhere until the next `show.load` replaces it. Outstanding leases
    /// go stale via the epoch bump and their `Drop` becomes a no-op — no
    /// double-return, no underflow.
    pub fn release_all(&self) {
        self.inner.active.store(0, Ordering::SeqCst);
        self.inner.peak.store(0, Ordering::SeqCst);
        self.inner.epoch.fetch_add(1, Ordering::SeqCst);
    }
}

/// A decoded video asset, resident and ready to sample.
#[derive(Debug)]
pub struct VideoAsset {
    pub asset_id: String,
    /// Frames in presentation order. For a VRAM-resident loop this is the
    /// whole period; when streaming it is the read-ahead window.
    pub frames: Vec<DecodedFrame>,
    pub width: u32,
    pub height: u32,
    /// The source's own frame rate, for cadence handling (SPEC §18).
    pub source_frame_rate: f32,
    /// The loop period. For a non-looping clip this is the frame count.
    pub period_frames: u32,
    pub plan: CachePlan,
    /// The decode session this asset holds for the show's duration ([RI-8]).
    /// Ownership IS the effect: the lease keeps `active` above zero while the
    /// asset is resident, and dropping the asset (load replacement, stop
    /// release) returns the slot. Never read — hence the prefix.
    _session: SessionLease,
}

impl VideoAsset {
    /// The frame to show for a source index, honouring the ring (SPEC §12.7).
    /// Beyond the resident window a streamed loop holds its last available
    /// frame rather than blocking the render thread (SPEC §12.8).
    pub fn frame_for(&self, source_index: u64) -> Option<&DecodedFrame> {
        if self.frames.is_empty() {
            return None;
        }
        let slot = loop_cache::texture_slot(source_index, self.period_frames)?;
        self.frames.get(slot).or_else(|| self.frames.last())
    }
}

/// Every video asset the loaded package needs.
#[derive(Debug, Default)]
pub struct VideoLibrary {
    pub assets: HashMap<String, VideoAsset>,
    /// Assets that failed to decode, with the reason. These are faults, and
    /// the caller reports each as `itemEvent: decodeError` (SPEC §5.9.3).
    pub failures: HashMap<String, String>,
}

impl VideoLibrary {
    pub fn get(&self, asset_id: &str) -> Option<&VideoAsset> {
        self.assets.get(asset_id)
    }
}

/// Decode one video asset and plan its residency.
///
/// `budget` and `gop_frames` come from the show's quality profile and the
/// asset's metadata; `declared_period` is `loop.periodFrames` when the
/// manifest declares one (SPEC §12.9 precedence: the manifest wins over the
/// decoded frame count).
pub fn load_video_asset(
    asset_id: &str,
    path: &Path,
    pool: &SessionPool,
    budget: CacheBudget,
    declared_period: Option<u32>,
    gop_frames: u32,
) -> Result<VideoAsset, DecodeError> {
    let lease = pool.acquire().ok_or_else(|| DecodeError::Failed {
        path: path.display().to_string(),
        reason: format!(
            "decode-session cap reached ({} active of {}); SPEC §24",
            pool.active(),
            pool.cap()
        ),
    })?;

    let mut session = DecodeSession::open(path)?;
    let source_frame_rate = session.nominal_frame_rate();

    // Decode a bounded number of frames: the plan needs the first frame's
    // dimensions, and the budget caps how many can be resident.
    let first = session.next_frame()?.ok_or_else(|| DecodeError::Failed {
        path: path.display().to_string(),
        reason: "no frames decoded".into(),
    })?;
    let (width, height) = (first.width, first.height);
    let has_alpha = first.rgba.as_chunks::<4>().0.iter().any(|px| px[3] != 255);

    // Plan against the declared period when there is one; otherwise decode to
    // find out, bounded by the budget.
    let provisional_period = declared_period.unwrap_or(u32::MAX);
    let plan = loop_cache::plan(
        LoopSpec {
            width,
            height,
            period_frames: provisional_period,
            has_alpha,
            yuv_sampling: false, // RGBA path until shader-side YUV lands
            gop_frames,
            // The engine plans against what it actually allocates, not what the
            // manifest asked for: this path decodes to RGBA8 regardless, so
            // passing a declared format here would make the plan describe a
            // residency the engine does not hold.
            //
            // The other half of that, which a reader needs to hear: preflight
            // plans the format the MANIFEST asked for (§12.11.1 reports
            // declared demand), so for ANY loop whose planned format is not
            // RGBA8 its number is an UNDER-estimate of what this engine holds,
            // and it can report a loop VRAM-resident that this path will
            // stream. That includes loops declaring nothing at all: preflight
            // passes `yuv_sampling: !has_alpha` and lands on NV12 where this
            // path lands on RGBA8, so a 60-frame 1080p loop with no
            // `textureFormat` reports 178 MiB there and holds 474 MiB here.
            // The undeclared case is the default case. Measured also for a
            // 200-frame `bc7` loop at `vramBudgetMib: 512`: 396 MiB reported,
            // streamed here. The smaller number is not the safe one until the
            // §12.3 ladder is real.
            declared_format: None,
        },
        budget,
    );

    // A declared loop is cached to the budget; a plain clip preloads a small
    // window and streams the rest (SPEC §12.8's frozen-frame rule covers the
    // gap until read-ahead lands).
    let cap = match declared_period {
        Some(_) => plan.max_frames_by_budget.max(1) as usize,
        None => CLIP_PRELOAD_FRAMES,
    };
    let mut frames = vec![first];
    while frames.len() < cap {
        match session.next_frame()? {
            Some(f) => frames.push(f),
            None => break,
        }
    }

    let period_frames = declared_period.unwrap_or(frames.len() as u32);
    // Re-plan now that the true period is known.
    let plan = loop_cache::plan(
        LoopSpec {
            width,
            height,
            period_frames,
            has_alpha,
            yuv_sampling: false,
            gop_frames,
            declared_format: None, // as above: what is allocated, not declared
        },
        budget,
    );

    // [RI-8] unload-at-next-load: a successfully decoded asset OWNS its
    // decode session until show.stop (or the next show.load's
    // release-then-rebuild) releases it. Failures drop the lease and free
    // the slot. No `mem::forget`: the lease lives in the asset, so a
    // load→load without a stop cannot stack sessions to the cap.
    Ok(VideoAsset {
        asset_id: asset_id.to_string(),
        frames,
        width,
        height,
        source_frame_rate,
        period_frames,
        plan,
        _session: lease,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_caps_sessions_and_counts_refusals() {
        let pool = SessionPool::with_cap(2);
        let a = pool.acquire().expect("first");
        let b = pool.acquire().expect("second");
        assert_eq!(pool.active(), 2);
        assert!(pool.acquire().is_none(), "the cap must refuse the third");
        assert_eq!(pool.refused(), 1);
        drop(a);
        assert_eq!(pool.active(), 1);
        let _c = pool.acquire().expect("a slot freed by drop is reusable");
        assert_eq!(pool.peak(), 2);
        drop(b);
    }

    #[test]
    fn stale_lease_after_release_all_does_not_underflow() {
        let pool = SessionPool::with_cap(2);
        let stale = pool.acquire().expect("first");
        pool.release_all();
        assert_eq!(pool.active(), 0);
        drop(stale); // reclaimed already: must be a silent no-op, not a wrap
        assert_eq!(pool.active(), 0);
        let _fresh = pool.acquire().expect("pool usable after release");
        assert_eq!(pool.active(), 1);
    }
}
