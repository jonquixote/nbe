//! Main-loop record feed (Prompt 09 WU-tap, SPEC §9.3).
//!
//! Runs AFTER `render_frame` and its deadline check, which are untouched: with
//! an active session it feeds one frame — RGBA (already read back by the
//! caller) → `encode_rgba` → `session.push_video`, tap drain → `session.push_audio`
//! — and reports its own time plus the caller's measured readback in
//! [`FeedOutcome::feed_ms`] (the `record_tap_ms` counter the loop accumulates
//! into engine state). That time is NEVER folded back into the render budget.
//!
//! The feed owns the ONE live encoder: it opens lazily at `VIEW_W/H` on the
//! first fed frame and captures the stream's real SPS/PPS from its first
//! keyframe into the session (the session starts with EMPTY sets — there is
//! no throwaway black-frame encode at any second geometry).
//!
//! Over-budget policy (degradation ladder: record yields, View never waits):
//! when the View's measured time already meets or exceeds the frame budget,
//! the feed SKIPS before touching any hardware — no open, no encode, no
//! pushes, the tap left for the next frame — and reports
//! [`FeedOutcome::skipped`]. The loop counts those skips in engine state and,
//! critically, skips the readback itself on the same condition, so a skipped
//! record frame costs nothing by definition.
//!
//! No threads, no file I/O: encode + bounded buffer pushes only. `finish` at
//! `record.stop` / `show.stop` quiescence still owns the file.

use std::time::{Duration, Instant};

use crate::encode::{EncodeSession, EncodedUnit};
use crate::record::{session::force_no_encoder, AudioTap, RecordSession};
use crate::render::{VIEW_H, VIEW_W};

/// Bitrate for the loop-owned encoder (content path only; the session carries
/// naming + geometry + buffers, never encoder config).
const RECORD_BITRATE: u32 = 8_000_000;

/// What one record feed did, for the loop's separate accounting and tests.
#[derive(Debug)]
pub struct FeedOutcome {
    /// Units encoded from this frame and buffered into the session.
    pub units: Vec<EncodedUnit>,
    /// Audio samples drained from the tap and buffered into the session.
    pub audio_samples: usize,
    /// Feed cost in milliseconds: the caller's measured readback PLUS the
    /// encode/push time here — the loop's `record_tap_ms` input, kept OFF the
    /// render budget by construction (it is measured here and only ever added
    /// to the record counter).
    pub feed_ms: f64,
    /// True when the record frame was skipped (over budget, or the encoder
    /// refused): nothing pushed, the tap untouched.
    pub skipped: bool,
    /// True when the encoder could not be opened (forced or genuinely
    /// absent). The loop latches this to stop retrying the open every frame.
    pub setup_failed: bool,
}

fn skipped() -> FeedOutcome {
    FeedOutcome {
        units: Vec::new(),
        audio_samples: 0,
        feed_ms: 0.0,
        skipped: true,
        setup_failed: false,
    }
}

/// Feed one recorded frame. `rgba` is the View readback the caller already
/// awaited; `readback_elapsed` is how long that await cost (folded into
/// `feed_ms`, never into the render budget); `render_elapsed` is the View's
/// measured time this frame and `budget` its frame budget (`None` while
/// STOPPED: always feed). `encoder` is the loop's live session, opened here
/// at `VIEW_W/H` on the first fed frame.
#[allow(clippy::too_many_arguments)]
pub fn feed_record_frame(
    rgba: &[u8],
    encoder: &mut Option<EncodeSession>,
    session: &mut RecordSession,
    tap: &AudioTap,
    render_elapsed: Duration,
    budget: Option<Duration>,
    readback_elapsed: Duration,
) -> FeedOutcome {
    // Degradation ladder: the View already spent the budget — yield before
    // touching any hardware (no open, no encode). Un-timed: a skip costs
    // nothing by definition.
    if let Some(b) = budget {
        if render_elapsed >= b {
            return skipped();
        }
    }
    let readback_ms = readback_elapsed.as_secs_f64() * 1000.0;
    let started = Instant::now();
    // The ONE live encoder, opened lazily at View geometry. Forced-unavailable
    // behaves exactly like missing hardware without an open attempt.
    if encoder.is_none() {
        if force_no_encoder() {
            return FeedOutcome {
                feed_ms: readback_ms + started.elapsed().as_secs_f64() * 1000.0,
                skipped: true,
                setup_failed: true,
                ..skipped()
            };
        }
        match EncodeSession::open(VIEW_W, VIEW_H, session.fps(), RECORD_BITRATE) {
            Ok(enc) => *encoder = Some(enc),
            Err(_) => {
                return FeedOutcome {
                    feed_ms: readback_ms + started.elapsed().as_secs_f64() * 1000.0,
                    skipped: true,
                    setup_failed: true,
                    ..skipped()
                };
            }
        }
    }
    let mut outcome = FeedOutcome {
        units: Vec::new(),
        audio_samples: 0,
        feed_ms: 0.0,
        skipped: false,
        setup_failed: false,
    };
    // Hardware encode is all-or-nothing by design (no CPU fallback): a
    // refusal degrades the record frame, never the View. The live encoder is
    // kept across the refusal — the next frame retries the feed, not the open.
    let units = match encoder
        .as_mut()
        .expect("encoder opened above")
        .encode_rgba(rgba)
    {
        Ok(units) => units,
        Err(_) => {
            outcome.skipped = true;
            outcome.feed_ms = readback_ms + started.elapsed().as_secs_f64() * 1000.0;
            return outcome;
        }
    };
    // First keyframe exposes the stream's real sets: capture them into the
    // metadata-only session (first capture wins; sets are stream parameters).
    // Units are buffered regardless — content is never held for the sets.
    if session.parameter_sets().is_none() {
        if let Some((sps, pps)) = encoder.as_ref().and_then(|e| e.parameter_sets()) {
            session.set_parameter_sets(sps, pps);
        }
    }
    for unit in &units {
        // The session cap refuses runaway buffers loudly; a refused unit ends
        // this frame's video without touching what is already buffered.
        if session.push_video(unit).is_err() {
            break;
        }
        outcome.units.push(unit.clone());
    }
    let drained = tap.drain();
    if !drained.is_empty() {
        // Audio is content, never silence: the session buffers it, refusing
        // loudly past its cap (the streaming thread's job, a later unit).
        if session.push_audio(&drained).is_ok() {
            outcome.audio_samples = drained.len();
        }
    }
    outcome.feed_ms = readback_ms + started.elapsed().as_secs_f64() * 1000.0;
    outcome
}
