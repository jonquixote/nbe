//! The master-bus limiter (SPEC §8.2; finding R26).
//!
//! The master stage was `(sample * gain).clamp(-1.0, 1.0)`: a clipper that
//! flat-topped every sample past full scale, under a comment that called it a
//! limiter. This is the limiter §8.2 requires, as the user ruled it on
//! 2026-10-21 (`docs/device-audio-design.md`, D4; the limiter plan's L1–L8):
//!
//! - **The ceiling** is the show's `truePeakDbtp` (default −1.5 dBTP), set at
//!   `show.load` (L1).
//! - **True peak** (L2): the detector oversamples 4× with a polyphase FIR,
//!   BS.1770-4 Annex 2's method: 48 taps, 12 per phase. The coefficients are a
//!   Kaiser-windowed sinc (β = 3.0), each phase normalised to unity DC gain.
//!   Measured over 100 Hz–20 kHz sines at random phase, it under-reads by at
//!   most 0.006 dB, and over-reads (limits early) by at most 0.27 dB.
//! - **A 5 ms lookahead** (L3), so the gain never moves faster than §8.7.1's
//!   5 ms minimum ramp. A full-depth reduction is a linear ramp across the
//!   whole window, and it is complete when the peak reaches the output.
//! - **Release** (L4): an exponential return to unity with time constant
//!   [`RELEASE_MS`].
//! - **Stereo-linked** (L5): one gain for both channels, so the image holds.
//!
//! The gain is the minimum of the required gain over the lookahead, averaged
//! over the same length. Every term of that average covers the sample it is
//! applied to, so the gain on any sample is at most `ceiling / true peak`
//! there. No output sample can exceed the ceiling, and no sample-step gain is
//! ever applied. Below the ceiling the gain is exactly 1.0, and the output is
//! the input delayed by [`MasterLimiter::latency_frames`], bit for bit.
//!
//! All state is allocated in [`MasterLimiter::new`]. `process` allocates
//! nothing and locks nothing (§8.9): it runs inside `AudioGraph::render`.

use crate::audio::{CHANNELS, SAMPLE_RATE};

// The limiter is stereo-linked over interleaved stereo.
const _: () = assert!(CHANNELS == 2, "the master limiter takes stereo frames");

/// §8.2's lookahead, as ruled (L3): 5 ms, §8.7.1's minimum ramp.
pub const LOOKAHEAD_MS: f64 = 5.0;
/// The release time constant (L4).
pub const RELEASE_MS: f64 = 100.0;
/// The ceiling when no show has set one: `show.audio.truePeakDbtp`'s default
/// (`schemas/manifest.v0.4.json`; SPEC §6.2).
pub const DEFAULT_CEILING_DBTP: f64 = -1.5;

/// The detector's oversampling: 4 phases of 12 taps (BS.1770-4 Annex 2).
const PHASES: usize = 4;
const TAPS: usize = 12;
/// The Kaiser window's β for the interpolation filter (see the module notes).
const KAISER_BETA: f64 = 3.0;
/// The filter's delay, in input samples: the interpolated points produced as
/// sample `n` arrives lie between `n − 6` and `n − 5`, so a sample's true
/// peak is complete six samples after it.
const DETECTOR_DELAY: usize = 6;
/// Below this distance from unity, a recovering gain is unity.
const UNITY_SNAP: f64 = 1e-7;

/// The 4× interpolation filter, one row per phase, each row summing to 1.
fn interpolation_phases() -> [[f64; TAPS]; PHASES] {
    let n = PHASES * TAPS;
    let m = (n - 1) as f64 / 2.0;
    // Cut off at the input's Nyquist frequency: 1/8 of the oversampled rate.
    let fc = 0.5 / PHASES as f64;
    let i0_beta = bessel_i0(KAISER_BETA);
    let mut h = [0.0f64; PHASES * TAPS];
    for (i, tap) in h.iter_mut().enumerate() {
        let x = i as f64 - m;
        let sinc = if x == 0.0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
        };
        let r = (1.0 - (x / m) * (x / m)).max(0.0);
        *tap = sinc * bessel_i0(KAISER_BETA * r.sqrt()) / i0_beta;
    }
    let mut phases = [[0.0f64; TAPS]; PHASES];
    for (k, phase) in phases.iter_mut().enumerate() {
        for (j, c) in phase.iter_mut().enumerate() {
            *c = h[PHASES * j + k];
        }
        let sum: f64 = phase.iter().sum();
        for c in phase.iter_mut() {
            *c /= sum;
        }
    }
    phases
}

/// The modified Bessel function of the first kind, order zero.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0f64, 1.0f64, 1.0f64);
    loop {
        term *= (x / 2.0) * (x / 2.0) / (k * k);
        sum += term;
        if term < 1e-15 * sum {
            return sum;
        }
        k += 1.0;
    }
}

/// The lookahead, stereo-linked, true-peak master limiter.
pub struct MasterLimiter {
    ceiling_dbtp: f64,
    ceiling: f64,
    /// The lookahead, in samples (5 ms at 48 kHz: 240).
    lookahead: usize,
    phases: [[f64; TAPS]; PHASES],
    /// The last `TAPS` input samples per channel, written twice so the newest
    /// `TAPS` are always the contiguous `hist_pos + 1 ..= hist_pos + TAPS`.
    hist: [[f64; 2 * TAPS]; 2],
    hist_pos: usize,
    /// Each recent sample's true peak (both channels), indexed by sample.
    tp: [f64; DETECTOR_DELAY + 2],
    /// A monotonic deque of (sample, required gain) for the sliding minimum.
    min_idx: Vec<u64>,
    min_val: Vec<f64>,
    min_head: usize,
    min_len: usize,
    /// The sliding minima, averaged over the lookahead.
    box_ring: Vec<f64>,
    box_pos: usize,
    box_sum: f64,
    box_not_unity: usize,
    /// The delayed audio, stereo interleaved.
    delay: Vec<f32>,
    delay_pos: usize,
    gain: f64,
    release_coeff: f64,
    /// Samples processed, offset so that every index the pipeline reads is
    /// non-negative; the state before it is silence.
    n: u64,
}

impl MasterLimiter {
    pub fn new(ceiling_dbtp: f64) -> Self {
        let lookahead = (SAMPLE_RATE as f64 * LOOKAHEAD_MS / 1000.0).round() as usize;
        let latency = lookahead + DETECTOR_DELAY - 1;
        let mut limiter = Self {
            ceiling_dbtp,
            ceiling: db_to_linear(ceiling_dbtp),
            lookahead,
            phases: interpolation_phases(),
            hist: [[0.0; 2 * TAPS]; 2],
            hist_pos: 0,
            tp: [0.0; DETECTOR_DELAY + 2],
            min_idx: vec![0; lookahead + 1],
            min_val: vec![1.0; lookahead + 1],
            min_head: 0,
            min_len: 0,
            box_ring: vec![1.0; lookahead],
            box_pos: 0,
            box_sum: lookahead as f64,
            box_not_unity: 0,
            delay: vec![0.0; latency * 2],
            delay_pos: 0,
            gain: 1.0,
            release_coeff: 1.0 - (-1000.0 / (RELEASE_MS * SAMPLE_RATE as f64)).exp(),
            n: DETECTOR_DELAY as u64,
        };
        limiter.set_ceiling_dbtp(ceiling_dbtp);
        limiter
    }

    /// Set the ceiling (L1: the show's `truePeakDbtp`, at `show.load`). A
    /// change reaches the output through the same lookahead ramp, never as a
    /// step.
    pub fn set_ceiling_dbtp(&mut self, dbtp: f64) {
        self.ceiling_dbtp = dbtp;
        self.ceiling = db_to_linear(dbtp);
    }

    /// The ceiling in force, in dBTP.
    pub fn ceiling_dbtp(&self) -> f64 {
        self.ceiling_dbtp
    }

    /// The lookahead, in samples.
    pub fn lookahead_frames(&self) -> usize {
        self.lookahead
    }

    /// The delay every sample takes through the limiter: the lookahead plus the
    /// true-peak filter's delay, less the sample it is applied to.
    pub fn latency_frames(&self) -> usize {
        self.delay.len() / 2
    }

    /// The gain applied to the frame most recently returned.
    pub fn gain(&self) -> f64 {
        self.gain
    }

    /// One stereo frame in; the frame `latency_frames()` earlier out, limited.
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let n = self.n;
        // The detector: the frame's own sample peak, then the four
        // interpolated points between `n − 6` and `n − 5`, which count toward
        // both of those samples.
        for (ch, v) in [left, right].into_iter().enumerate() {
            self.hist[ch][self.hist_pos] = v as f64;
            self.hist[ch][self.hist_pos + TAPS] = v as f64;
        }
        let mut between = 0.0f64;
        for hist in &self.hist {
            let newest = self.hist_pos + TAPS;
            for phase in &self.phases {
                let mut y = 0.0;
                for (j, c) in phase.iter().enumerate() {
                    y += c * hist[newest - j];
                }
                between = between.max(y.abs());
            }
        }
        self.hist_pos = (self.hist_pos + 1) % TAPS;
        let slots = self.tp.len() as u64;
        self.tp[(n % slots) as usize] = (left.abs() as f64).max(right.abs() as f64);
        for back in [DETECTOR_DELAY as u64, DETECTOR_DELAY as u64 - 1] {
            let slot = ((n - back) % slots) as usize;
            self.tp[slot] = self.tp[slot].max(between);
        }

        // Sample `s` is complete: its required gain joins the sliding minimum.
        let s = n - DETECTOR_DELAY as u64;
        let peak = self.tp[(s % slots) as usize];
        let required = if peak <= self.ceiling {
            1.0
        } else {
            self.ceiling / peak
        };
        self.push_min(s, required);
        let minimum = self.min_val[self.min_head];

        // Average the minima over the lookahead: a ramp, never a step.
        let old = self.box_ring[self.box_pos];
        if old != 1.0 {
            self.box_not_unity -= 1;
        }
        if minimum != 1.0 {
            self.box_not_unity += 1;
        }
        self.box_ring[self.box_pos] = minimum;
        self.box_pos = (self.box_pos + 1) % self.lookahead;
        self.box_sum += minimum - old;
        if self.box_not_unity == 0 {
            // Exactly unity again: no rounding left over from the episode.
            self.box_sum = self.lookahead as f64;
        } else if self.box_pos == 0 {
            // A long episode: re-derive the sum once a window, so rounding
            // cannot accumulate.
            self.box_sum = self.box_ring.iter().sum();
        }
        let attack = self.box_sum / self.lookahead as f64;

        // Down at once along the attack ramp; back up at the release rate.
        let released = self.gain + (1.0 - self.gain) * self.release_coeff;
        let mut gain = attack.min(released);
        if gain > 1.0 - UNITY_SNAP && attack >= 1.0 {
            gain = 1.0;
        }
        self.gain = gain;

        // The delayed frame out, with the gain that belongs to it.
        let dl = self.delay[self.delay_pos];
        let dr = self.delay[self.delay_pos + 1];
        self.delay[self.delay_pos] = left;
        self.delay[self.delay_pos + 1] = right;
        self.delay_pos = (self.delay_pos + 2) % self.delay.len();
        self.n += 1;
        if gain == 1.0 {
            (dl, dr)
        } else {
            ((dl as f64 * gain) as f32, (dr as f64 * gain) as f32)
        }
    }

    /// Push sample `s`'s required gain; drop what left the window.
    fn push_min(&mut self, s: u64, value: f64) {
        let cap = self.min_idx.len();
        while self.min_len > 0 {
            let back = (self.min_head + self.min_len - 1) % cap;
            if self.min_val[back] >= value {
                self.min_len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.min_head + self.min_len) % cap;
        self.min_idx[slot] = s;
        self.min_val[slot] = value;
        self.min_len += 1;
        let oldest = s.saturating_sub(self.lookahead as u64 - 1);
        while self.min_idx[self.min_head] < oldest {
            self.min_head = (self.min_head + 1) % cap;
            self.min_len -= 1;
        }
    }
}

fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phase_of_the_interpolator_has_unity_dc_gain() {
        for phase in interpolation_phases() {
            let sum: f64 = phase.iter().sum();
            assert!((sum - 1.0).abs() < 1e-12, "a phase sums to {sum}");
        }
    }

    #[test]
    fn the_latency_is_the_lookahead_plus_the_detector() {
        let l = MasterLimiter::new(DEFAULT_CEILING_DBTP);
        assert_eq!(l.lookahead_frames(), 240, "5 ms at 48 kHz");
        assert_eq!(l.latency_frames(), 240 + DETECTOR_DELAY - 1);
    }
}
