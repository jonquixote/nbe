//! The master limiter (SPEC §8.2 and its note, v0.4.8 row 14; finding R26).
//!
//! Every test renders the real graph offline (`AudioGraph::render`): no
//! device and no wall clock, so each claim is a measurement over samples and
//! holds on any machine. The ceiling is the spec default, −1.5 dBTP, unless a
//! test sets the show's.
//!
//! True peak is measured here by the test's own meter: 8× oversampled with a
//! long Kaiser-windowed sinc (64 taps per phase). It is independent of the
//! limiter's 4× detector, so a detector that under-reads cannot hide behind
//! itself.

use nbe_engine::audio::{AudioGraph, BusId, Source, CHANNELS, SAMPLE_RATE};
use std::sync::Arc;

const HOUSE_RATE: u32 = 30;
/// One house frame's block, as the audio driver renders it.
const BLOCK: usize = SAMPLE_RATE as usize / HOUSE_RATE as usize;
/// The spec default ceiling, −1.5 dBTP, linear.
fn ceiling() -> f64 {
    10f64.powf(-1.5 / 20.0)
}

/// Render `frames` sample frames, block by block on the master clock.
fn render(g: &mut AudioGraph, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames * CHANNELS);
    let mut buf = vec![0.0f32; BLOCK * CHANNELS];
    let mut frame = 0u64;
    while out.len() < frames * CHANNELS {
        g.render(&mut buf, frame);
        out.extend_from_slice(&buf);
        frame += 1;
    }
    out.truncate(frames * CHANNELS);
    out
}

fn channel(interleaved: &[f32], c: usize) -> Vec<f64> {
    interleaved
        .iter()
        .skip(c)
        .step_by(CHANNELS)
        .map(|&v| v as f64)
        .collect()
}

fn sample_peak(x: &[f32]) -> f64 {
    x.iter().fold(0.0f64, |m, &v| m.max(v.abs() as f64))
}

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

/// The test's true-peak meter: 8× oversampled, 64 taps per phase.
fn true_peak(x: &[f64]) -> f64 {
    const L: usize = 8;
    const TAPS: usize = 64;
    let n = L * TAPS;
    let m = (n - 1) as f64 / 2.0;
    let fc = 0.5 / L as f64;
    let beta = 8.0;
    let mut phases = vec![vec![0.0f64; TAPS]; L];
    for i in 0..n {
        let t = i as f64 - m;
        let sinc = if t == 0.0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * t).sin() / (std::f64::consts::PI * t)
        };
        let w = bessel_i0(beta * (1.0 - (t / m) * (t / m)).max(0.0).sqrt()) / bessel_i0(beta);
        phases[i % L][i / L] = sinc * w;
    }
    for p in phases.iter_mut() {
        let s: f64 = p.iter().sum();
        p.iter_mut().for_each(|c| *c /= s);
    }
    let mut peak = x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    for k in TAPS..x.len() {
        for p in &phases {
            let y: f64 = (0..TAPS).map(|j| p[j] * x[k - j]).sum();
            peak = peak.max(y.abs());
        }
    }
    peak
}

fn db(v: f64) -> f64 {
    20.0 * v.log10()
}

/// Goertzel power at `hz` over `x`.
fn goertzel(x: &[f64], hz: f64) -> f64 {
    let k = 2.0 * (2.0 * std::f64::consts::PI * hz / SAMPLE_RATE as f64).cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s = v + k * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    s1 * s1 + s2 * s2 - k * s1 * s2
}

/// The longest run of consecutive samples sharing one magnitude: a clamp's
/// flat top, whatever level it clamps at.
fn longest_flat_run(x: &[f64]) -> usize {
    let (mut best, mut run) = (1usize, 1usize);
    for w in x.windows(2) {
        if (w[0].abs() - w[1].abs()).abs() < 1e-9 && w[0].abs() > 1e-3 {
            run += 1;
            best = best.max(run);
        } else {
            run = 1;
        }
    }
    best
}

fn pcm(samples: Vec<f32>) -> Vec<Source> {
    vec![Source::Pcm {
        samples: Arc::new(samples),
        t0: 0,
        looping: true,
    }]
}

/// A deterministic white noise, interleaved stereo, in [-amp, amp].
fn noise(frames: usize, amp: f32) -> Vec<f32> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    (0..frames * CHANNELS)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32 * amp
        })
        .collect()
}

// F1 — a hot master never leaves above the ceiling.

/// One hot signal, set up on a fresh graph.
type Setup = Box<dyn Fn(&mut AudioGraph)>;

#[test]
fn a_hot_master_never_leaves_above_the_ceiling() {
    let c = ceiling();
    let cases: Vec<(&str, Setup)> = vec![
        (
            "four buses at 0.8 (+10.1 dBFS summed)",
            Box::new(|g: &mut AudioGraph| {
                for id in [BusId::Mic, BusId::Clip, BusId::Music, BusId::Guest] {
                    g.set_source(
                        id,
                        vec![Source::Tone {
                            hz: 220.0,
                            amplitude: 0.8,
                        }],
                    );
                }
            }),
        ),
        (
            "a full-scale sine at +12 dB bus gain",
            Box::new(|g: &mut AudioGraph| {
                g.set_source(
                    BusId::Music,
                    vec![Source::Tone {
                        hz: 997.0,
                        amplitude: 1.0,
                    }],
                );
                g.bus_mut(BusId::Music).set_gain_db(12.0, 5.0);
            }),
        ),
        (
            "a single +20 dBFS impulse",
            Box::new(|g: &mut AudioGraph| {
                let mut s = vec![0.0f32; SAMPLE_RATE as usize * CHANNELS];
                s[1000 * CHANNELS] = 10.0;
                s[1000 * CHANNELS + 1] = -10.0;
                g.set_source(BusId::Sfx, pcm(s));
            }),
        ),
        (
            "white noise at +12 dB bus gain",
            Box::new(|g: &mut AudioGraph| {
                g.set_source(BusId::Clip, pcm(noise(SAMPLE_RATE as usize, 1.0)));
                g.bus_mut(BusId::Clip).set_gain_db(12.0, 5.0);
            }),
        ),
    ];
    for (name, setup) in cases {
        let mut g = AudioGraph::new(HOUSE_RATE);
        setup(&mut g);
        let out = render(&mut g, SAMPLE_RATE as usize);
        let peak = sample_peak(&out);
        assert!(
            peak > 0.5,
            "{name}: precondition, the signal must be loud, got {peak:.3}"
        );
        assert!(
            peak <= c * (1.0 + 1e-6),
            "{name}: a sample left at {:.2} dBFS, over the {:.2} dBTP ceiling",
            db(peak),
            db(c)
        );
    }
}

// F2 — the inter-sample stimulus: sample peak under the ceiling, true peak
// over it. A sample-peak limiter passes it untouched; a true-peak one must not.

#[test]
fn an_inter_sample_peak_over_the_ceiling_is_caught() {
    let c = ceiling();
    // fs/4 at a 45° phase: the samples sit at ±0.7071 of the true peak.
    let amp = 1.0f64;
    let frames = SAMPLE_RATE as usize;
    let mut s = Vec::with_capacity(frames * CHANNELS);
    for n in 0..frames {
        let v = (amp
            * (2.0 * std::f64::consts::PI * n as f64 / 4.0 + std::f64::consts::FRAC_PI_4).sin())
            as f32;
        s.push(v);
        s.push(v);
    }
    let input_left = channel(&s, 0);
    let in_sample = input_left.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let in_true = true_peak(&input_left[..4800]);
    assert!(
        in_sample < c && in_true > c,
        "precondition, the stimulus: sample peak {:.2} dBFS under the {:.2} ceiling, true peak {:.2} dBTP over it",
        db(in_sample),
        db(c),
        db(in_true)
    );

    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(BusId::Music, pcm(s));
    let out = render(&mut g, frames);
    // The last quarter second: well past the attack.
    let tail = channel(&out[(frames - 12_000) * CHANNELS..], 0);
    let out_true = true_peak(&tail);
    assert!(
        db(out_true) <= db(c) + 0.1,
        "the true peak left at {:.2} dBTP, over the {:.2} dBTP ceiling (+0.1 dB allowed); its samples peak at {:.2} dBFS",
        db(out_true),
        db(c),
        db(tail.iter().fold(0.0f64, |m, v| m.max(v.abs())))
    );
}

// F3 — no hard clip, ever: a steady hot sine leaves as a sine.

#[test]
fn a_steady_hot_sine_leaves_as_a_sine_not_a_flat_top() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    // +10 dBFS at 1 kHz: 48 samples per period, so the bins are exact.
    g.set_source(
        BusId::Music,
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: 3.162,
        }],
    );
    let out = render(&mut g, 2 * SAMPLE_RATE as usize);
    // The second second: the attack settled long before it.
    let tail = channel(&out[SAMPLE_RATE as usize * CHANNELS..], 0);
    let flat = longest_flat_run(&tail);
    assert!(
        flat < 3,
        "a run of {flat} samples at one magnitude: the master stage flat-tops"
    );
    let fundamental = goertzel(&tail, 1000.0);
    for h in 2..=9 {
        let rel = 10.0 * (goertzel(&tail, 1000.0 * h as f64) / fundamental).log10();
        assert!(
            rel <= -60.0,
            "harmonic {h} sits at {rel:.1} dB under the fundamental: the limited sine is distorted"
        );
    }
}
