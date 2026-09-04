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
use std::sync::atomic::{AtomicU32, Ordering};

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
    active: AtomicU32,
    peak: AtomicU32,
    refused: AtomicU32,
    cap: u32,
}

/// A live lease on a decode session. Dropping it returns the slot.
pub struct SessionLease<'a> {
    pool: &'a SessionPool,
}

impl Drop for SessionLease<'_> {
    fn drop(&mut self) {
        self.pool.active.fetch_sub(1, Ordering::SeqCst);
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
    pub fn acquire(&self) -> Option<SessionLease<'_>> {
        loop {
            let current = self.active.load(Ordering::SeqCst);
            if current >= self.cap {
                self.refused.fetch_add(1, Ordering::SeqCst);
                return None;
            }
            if self
                .active
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                self.peak.fetch_max(current + 1, Ordering::SeqCst);
                return Some(SessionLease { pool: self });
            }
        }
    }

    pub fn active(&self) -> u32 {
        self.active.load(Ordering::SeqCst)
    }

    pub fn peak(&self) -> u32 {
        self.peak.load(Ordering::SeqCst)
    }

    pub fn refused(&self) -> u32 {
        self.refused.load(Ordering::SeqCst)
    }

    pub fn cap(&self) -> u32 {
        self.cap
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
    let _lease = pool.acquire().ok_or_else(|| DecodeError::Failed {
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
        },
        budget,
    );

    Ok(VideoAsset {
        asset_id: asset_id.to_string(),
        frames,
        width,
        height,
        source_frame_rate,
        period_frames,
        plan,
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
}
