//! Loop cache planning and residency (Prompt 05 Step 5, SPEC §12).
//!
//! The order is mandated (§26 sequencing): **format accounting first**, then
//! the budget, then the policy. `frameCostMiB` comes from the selected texture
//! format; `maxFramesByBudget` comes from the effective budget; only then does
//! a loop become VRAM-resident or streamed.
//!
//! This module is pure: no GPU, no decode, no I/O. That is deliberate — the
//! budget math is the part AC-21 measures, and it is measurable without
//! hardware.

use serde::{Deserialize, Serialize};

/// Texture formats a cached loop frame can occupy (SPEC §12.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CacheTextureFormat {
    /// Two-plane YUV: `R8Unorm` luma + `Rg8Unorm` chroma, BT.709 conversion in
    /// the shader (v0.2.1 errata). 1.5 bytes per pixel.
    Nv12,
    /// Two-plane YUV plus a full-resolution alpha plane. 2.5 bytes per pixel.
    Nv12Alpha,
    /// Straight RGBA8. 4 bytes per pixel.
    Rgba8,
}

impl CacheTextureFormat {
    /// Bytes per pixel for one cached frame.
    pub fn bytes_per_pixel(&self) -> f64 {
        match self {
            CacheTextureFormat::Nv12 => 1.5,
            CacheTextureFormat::Nv12Alpha => 2.5,
            CacheTextureFormat::Rgba8 => 4.0,
        }
    }

    /// The format a source gets: alpha forces an alpha-carrying format, and
    /// without hardware YUV sampling the engine falls back to RGBA8.
    pub fn select(has_alpha: bool, yuv_sampling: bool) -> Self {
        match (has_alpha, yuv_sampling) {
            (true, true) => CacheTextureFormat::Nv12Alpha,
            (false, true) => CacheTextureFormat::Nv12,
            _ => CacheTextureFormat::Rgba8,
        }
    }
}

/// What the cache decided to do with a loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CachePolicy {
    /// Every frame is resident in VRAM; wrap is a modulo, never a restart.
    Vram,
    /// The loop does not fit: only the read-ahead window is resident.
    Streaming,
}

/// The plan for one loop, in the order SPEC §26 mandates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachePlan {
    pub selected_texture_format: CacheTextureFormat,
    pub frame_cost_mib: f64,
    pub effective_budget_mib: u32,
    pub max_frames_by_budget: u32,
    pub period_frames: u32,
    pub cache_policy_selected: CachePolicy,
    pub vram_resident: bool,
    /// Minimum frames held ahead when streaming (SPEC §12.8).
    pub read_ahead_frames: u32,
    pub reason: String,
}

/// Inputs to the plan. Everything here is known before a frame is decoded.
#[derive(Debug, Clone, Copy)]
pub struct LoopSpec {
    pub width: u32,
    pub height: u32,
    pub period_frames: u32,
    pub has_alpha: bool,
    pub yuv_sampling: bool,
    /// GOP length, for the §12.8 read-ahead minimum.
    pub gop_frames: u32,
}

/// The budget a loop is planned against.
#[derive(Debug, Clone, Copy)]
pub struct CacheBudget {
    /// Per-loop ceiling in MiB.
    pub per_loop_mib: u32,
    /// Total cache ceiling in MiB, shared across loops.
    pub total_mib: u32,
    /// The adapter's recommended working-set size in MiB, if known. On Apple
    /// unified memory this clamps the budget (SPEC §12.6) — VRAM is the same
    /// silicon the rest of the show is running on.
    pub recommended_working_set_mib: Option<u32>,
}

impl CacheBudget {
    /// The effective per-loop budget after the unified-memory clamp.
    ///
    /// SPEC §12.6: on Apple silicon the cache may not assume dedicated VRAM.
    /// The clamp is a quarter of the recommended working set, so a loop cache
    /// cannot crowd out decode, compositing, and the encoder.
    pub fn effective_mib(&self) -> u32 {
        let base = self.per_loop_mib.min(self.total_mib);
        match self.recommended_working_set_mib {
            Some(ws) => base.min(ws / 4),
            None => base,
        }
    }
}

/// Plan a loop's residency. Format accounting first, then budget, then policy.
pub fn plan(spec: LoopSpec, budget: CacheBudget) -> CachePlan {
    // 1. Format accounting.
    let format = CacheTextureFormat::select(spec.has_alpha, spec.yuv_sampling);
    let bytes_per_frame = spec.width as f64 * spec.height as f64 * format.bytes_per_pixel();
    let frame_cost_mib = bytes_per_frame / (1024.0 * 1024.0);

    // 2. Budget.
    let effective_budget_mib = budget.effective_mib();
    let max_frames_by_budget = if frame_cost_mib <= 0.0 {
        0
    } else {
        (effective_budget_mib as f64 / frame_cost_mib).floor() as u32
    };

    // 3. Policy.
    let fits = spec.period_frames > 0 && spec.period_frames <= max_frames_by_budget;
    let read_ahead_frames = (2 * spec.gop_frames).max(60);
    let (cache_policy_selected, reason) = if fits {
        (
            CachePolicy::Vram,
            format!(
                "{} frames x {:.3} MiB = {:.1} MiB fits the {} MiB effective budget",
                spec.period_frames,
                frame_cost_mib,
                spec.period_frames as f64 * frame_cost_mib,
                effective_budget_mib
            ),
        )
    } else {
        (
            CachePolicy::Streaming,
            format!(
                "{} frames x {:.3} MiB exceeds the {} MiB effective budget ({} frames max); streaming with {} frames read-ahead",
                spec.period_frames,
                frame_cost_mib,
                effective_budget_mib,
                max_frames_by_budget,
                read_ahead_frames
            ),
        )
    };

    CachePlan {
        selected_texture_format: format,
        frame_cost_mib,
        effective_budget_mib,
        max_frames_by_budget,
        period_frames: spec.period_frames,
        cache_policy_selected,
        vram_resident: fits,
        read_ahead_frames,
        reason,
    }
}

/// The VRAM ring buffer (SPEC §12.7).
///
/// `textureSlot = sourceIndex mod P`. There is no decoder restart at wrap and
/// no branch that distinguishes the wrap frame from any other — which is
/// exactly what AC-9 checks.
pub fn texture_slot(source_index: u64, period_frames: u32) -> Option<usize> {
    if period_frames == 0 {
        return None;
    }
    Some((source_index % period_frames as u64) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_1080p(period: u32) -> LoopSpec {
        LoopSpec {
            width: 1920,
            height: 1080,
            period_frames: period,
            has_alpha: false,
            yuv_sampling: true,
            gop_frames: 30,
        }
    }

    #[test]
    fn frame_cost_follows_the_selected_format() {
        // 1920x1080 NV12 = 1920*1080*1.5 bytes = 2.966 MiB.
        let p = plan(
            spec_1080p(100),
            CacheBudget {
                per_loop_mib: 1024,
                total_mib: 4096,
                recommended_working_set_mib: None,
            },
        );
        assert_eq!(p.selected_texture_format, CacheTextureFormat::Nv12);
        assert!(
            (p.frame_cost_mib - 2.9663).abs() < 0.001,
            "frame cost was {}",
            p.frame_cost_mib
        );
    }

    #[test]
    fn alpha_selects_an_alpha_format_and_costs_more() {
        let mut spec = spec_1080p(10);
        spec.has_alpha = true;
        let p = plan(
            spec,
            CacheBudget {
                per_loop_mib: 1024,
                total_mib: 4096,
                recommended_working_set_mib: None,
            },
        );
        assert_eq!(p.selected_texture_format, CacheTextureFormat::Nv12Alpha);
        assert!(p.frame_cost_mib > 4.9 && p.frame_cost_mib < 5.0);
    }

    #[test]
    fn a_loop_that_fits_is_vram_resident() {
        let p = plan(
            spec_1080p(100),
            CacheBudget {
                per_loop_mib: 1024,
                total_mib: 4096,
                recommended_working_set_mib: None,
            },
        );
        assert_eq!(p.cache_policy_selected, CachePolicy::Vram);
        assert!(p.vram_resident);
        assert_eq!(p.max_frames_by_budget, 345); // floor(1024 / 2.9663)
    }

    #[test]
    fn a_loop_that_does_not_fit_streams_with_the_mandated_read_ahead() {
        let p = plan(
            spec_1080p(5000),
            CacheBudget {
                per_loop_mib: 1024,
                total_mib: 4096,
                recommended_working_set_mib: None,
            },
        );
        assert_eq!(p.cache_policy_selected, CachePolicy::Streaming);
        assert!(!p.vram_resident);
        // SPEC §12.8: max(2 * GOP, 60).
        assert_eq!(p.read_ahead_frames, 60);
    }

    #[test]
    fn the_apple_unified_memory_clamp_lowers_the_budget() {
        // SPEC §12.6: the cache may not assume dedicated VRAM.
        let unclamped = plan(
            spec_1080p(100),
            CacheBudget {
                per_loop_mib: 1024,
                total_mib: 4096,
                recommended_working_set_mib: None,
            },
        );
        let clamped = plan(
            spec_1080p(100),
            CacheBudget {
                per_loop_mib: 1024,
                total_mib: 4096,
                recommended_working_set_mib: Some(2048),
            },
        );
        assert_eq!(clamped.effective_budget_mib, 512);
        assert!(clamped.max_frames_by_budget < unclamped.max_frames_by_budget);
    }

    #[test]
    fn the_ring_slot_is_a_modulo_with_no_wrap_special_case() {
        let period = 10;
        for i in 0..40u64 {
            assert_eq!(texture_slot(i, period), Some((i % 10) as usize));
        }
        assert_eq!(texture_slot(5, 0), None);
    }
}
