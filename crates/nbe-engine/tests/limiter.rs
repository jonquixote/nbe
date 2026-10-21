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

// ---------------------------------------------------------------------------
// The limiter as built: transparency, the attack ramp, the release constant,
// the stereo link, the latency, and the show's ceiling (§8.2's note).
// ---------------------------------------------------------------------------

use nbe_engine::limiter::{MasterLimiter, DEFAULT_CEILING_DBTP, LOOKAHEAD_MS, RELEASE_MS};

/// Interleaved stereo from one channel's samples, both channels alike.
fn stereo(mono: &[f32]) -> Vec<f32> {
    mono.iter().flat_map(|&v| [v, v]).collect()
}

fn sine(hz: f64, amp: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            (amp * (2.0 * std::f64::consts::PI * hz * n as f64 / SAMPLE_RATE as f64).sin()) as f32
        })
        .collect()
}

// F4 — below the ceiling the limiter is transparent: the input, delayed, bit
// for bit.

#[test]
fn below_the_ceiling_the_output_is_the_input_delayed_bit_for_bit() {
    let frames = SAMPLE_RATE as usize / 2;
    let quiet = sine(1000.0, 0.1, frames);
    let chord: Vec<f32> = (0..frames)
        .map(|n| {
            let t = n as f64 / SAMPLE_RATE as f64;
            let v = [261.63, 329.63, 392.0]
                .iter()
                .map(|f| (2.0 * std::f64::consts::PI * f * t).sin())
                .sum::<f64>()
                / 3.0
                * 0.5;
            v as f32
        })
        .collect();
    for (name, mono) in [("a −20 dBFS sine", quiet), ("a −6 dBFS chord", chord)] {
        let input = stereo(&mono);
        let mut g = AudioGraph::new(HOUSE_RATE);
        g.set_source(
            BusId::Music,
            vec![Source::Pcm {
                samples: Arc::new(input.clone()),
                t0: 0,
                looping: false,
            }],
        );
        let lat = g.latency_frames();
        let out = render(&mut g, frames);
        for f in 0..frames - lat {
            for c in 0..CHANNELS {
                let (o, i) = (out[(f + lat) * CHANNELS + c], input[f * CHANNELS + c]);
                assert!(
                    o.to_bits() == i.to_bits(),
                    "{name}: frame {f} channel {c} left as {o:e}, not {i:e} delayed by {lat}"
                );
            }
        }
        assert!(
            out[..lat * CHANNELS].iter().all(|&v| v == 0.0),
            "{name}: the delay line starts silent"
        );
    }
}

// F5 — the attack is a ramp, never a step: no faster than §8.7.1's 5 ms.

#[test]
fn the_attack_ramps_over_the_whole_5_ms_lookahead() {
    let mut l = MasterLimiter::new(DEFAULT_CEILING_DBTP);
    let lookahead = l.lookahead_frames();
    assert_eq!(
        lookahead,
        (SAMPLE_RATE as f64 * LOOKAHEAD_MS / 1000.0).round() as usize,
        "the lookahead is §8.2's 5 ms"
    );
    // Half a second of silence, then a +6 dBFS sine.
    let burst = sine(1000.0, 2.0, SAMPLE_RATE as usize / 2);
    let mut gains = Vec::new();
    for _ in 0..SAMPLE_RATE / 2 {
        l.process(0.0, 0.0);
        gains.push(l.gain());
    }
    for &v in &burst {
        l.process(v, v);
        gains.push(l.gain());
    }
    let start = gains
        .iter()
        .position(|&g| g < 1.0)
        .expect("the burst must reduce the gain");
    let floor = gains.iter().cloned().fold(1.0f64, f64::min);
    let reach = gains[start..]
        .iter()
        .position(|&g| g <= floor + 1e-12)
        .expect("the gain reaches its floor");
    let steepest = gains.windows(2).map(|w| w[0] - w[1]).fold(0.0f64, f64::max);
    assert!(
        reach + 1 >= lookahead,
        "the reduction to {floor:.4} took {} samples: faster than the {lookahead}-sample (5 ms) ramp §8.7.1 allows",
        reach + 1
    );
    assert!(
        steepest <= (1.0 - floor) / lookahead as f64 * (1.0 + 1e-9),
        "a per-sample gain step of {steepest:.6}: the attack steps, it does not ramp"
    );
    assert!(
        gains[start..start + reach]
            .windows(2)
            .all(|w| w[1] <= w[0] + 1e-15),
        "the attack is monotone"
    );
}

// F6 — release: back to unity with the 100 ms time constant, then
// transparent again.

#[test]
fn the_gain_recovers_with_the_100_ms_release_constant() {
    assert_eq!(RELEASE_MS, 100.0, "the release time constant ruled (L4)");
    let mut l = MasterLimiter::new(DEFAULT_CEILING_DBTP);
    for &v in &sine(1000.0, 2.0, SAMPLE_RATE as usize / 4) {
        l.process(v, v);
    }
    // Quiet from here: the window empties of overs, then the release runs.
    let quiet = sine(1000.0, 0.1, 3 * SAMPLE_RATE as usize);
    let mut gains = Vec::new();
    for &v in &quiet {
        l.process(v, v);
        gains.push(l.gain());
    }
    // The release starts where the gain first rises.
    let start = gains
        .windows(2)
        .position(|w| w[1] > w[0])
        .expect("the gain releases");
    let g0 = gains[start];
    let tau = (SAMPLE_RATE as f64 * RELEASE_MS / 1000.0) as usize;
    let at_tau = gains[start + tau];
    let expected = 1.0 - (1.0 - g0) * (-1.0f64).exp();
    assert!(
        (at_tau - expected).abs() <= 0.01 * (1.0 - g0),
        "one time constant into the release the gain is {at_tau:.4}; a 100 ms constant gives {expected:.4}"
    );
    let at_5tau = gains[start + 5 * tau];
    assert!(
        20.0 * at_5tau.log10() >= -0.1,
        "five time constants in, the gain is {:.3} dB, not within 0.1 dB of unity",
        20.0 * at_5tau.log10()
    );
    assert_eq!(
        *gains.last().unwrap(),
        1.0,
        "recovered: the gain is exactly unity again, so the limiter is transparent"
    );
}

// F7 — stereo-linked: one gain for both channels.

#[test]
fn a_hot_left_channel_reduces_the_right_by_the_same_gain() {
    let frames = SAMPLE_RATE as usize / 2;
    let left = sine(1000.0, 2.0, frames);
    let right = sine(1000.0, 0.1, frames);
    let input: Vec<f32> = left
        .iter()
        .zip(&right)
        .flat_map(|(&l, &r)| [l, r])
        .collect();
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Music,
        vec![Source::Pcm {
            samples: Arc::new(input.clone()),
            t0: 0,
            looping: false,
        }],
    );
    let lat = g.latency_frames();
    let out = render(&mut g, frames);
    // Late in the burst, where both channels are well away from zero.
    for f in (frames / 2..frames - lat).step_by(97) {
        let (il, ir) = (input[f * 2] as f64, input[f * 2 + 1] as f64);
        if il.abs() < 0.5 || ir.abs() < 0.02 {
            continue;
        }
        let gl = out[(f + lat) * 2] as f64 / il;
        let gr = out[(f + lat) * 2 + 1] as f64 / ir;
        assert!(
            gl < 0.9 && (gl - gr).abs() < 1e-5,
            "frame {f}: the left is reduced to {gl:.5} and the right to {gr:.5}; a linked limiter reduces both alike"
        );
    }
}

// F9 — the latency is exact, and reported.

#[test]
fn the_reported_latency_is_the_delay_every_sample_takes() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    let lat = g.latency_frames();
    assert_eq!(
        lat,
        240 + 5,
        "5 ms of lookahead (240) plus the true-peak filter's 5 samples"
    );
    let mut s = vec![0.0f32; SAMPLE_RATE as usize / 10 * CHANNELS];
    let at = 1000;
    s[at * CHANNELS] = 0.5;
    s[at * CHANNELS + 1] = 0.5;
    g.set_source(
        BusId::Sfx,
        vec![Source::Pcm {
            samples: Arc::new(s),
            t0: 0,
            looping: false,
        }],
    );
    let out = render(&mut g, SAMPLE_RATE as usize / 10);
    let first = out
        .chunks(CHANNELS)
        .position(|f| f[0] != 0.0)
        .expect("the impulse arrives");
    assert_eq!(
        first,
        at + lat,
        "the impulse arrives exactly latency_frames() late"
    );
    assert_eq!(
        out[first * CHANNELS],
        0.5,
        "and unchanged: it is under the ceiling"
    );
}

// L1 — the ceiling is the show's `truePeakDbtp`, in force from `show.load`.

fn load_show(true_peak: Option<f64>) -> Vec<nbe_engine::audio_control::AudioCommand> {
    use nbe_engine::directive::DirectiveHandler;
    use nbe_engine::state::{EngineState, OutgoingQueue};
    use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
    let dir = tempfile::tempdir().unwrap();
    // The engine's load wants a fallback slate it can decode.
    std::fs::create_dir_all(dir.path().join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        64,
        32,
        image::Rgba([10, 20, 30, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(dir.path().join("media/slate.png"), &png).unwrap();
    let mut audio = serde_json::json!({ "sampleRate": 48000, "loudnessTargetLufs": -16 });
    if let Some(tp) = true_peak {
        audio["truePeakDbtp"] = serde_json::json!(tp);
    }
    let manifest = serde_json::json!({
        "manifestVersion": "0.4",
        "network": { "id": "nbe", "name": "T" },
        "show": {
            "id": "s", "title": "T",
            "video": {"width": 1920, "height": 1080, "frameRate": 30, "colorSpace": "rec709"},
            "audio": audio,
            "fallbackAssetId": "slate"
        },
        "assets": [{ "id": "slate", "kind": "image", "source": "media/slate.png" }],
        "scenes": [],
        "rundown": { "id": "R", "items": [] }
    });
    std::fs::write(dir.path().join("manifest.json"), manifest.to_string()).unwrap();
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    let frame = DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: 1,
        state_version: 1,
        command: "show.load".into(),
        target: serde_json::json!({}),
        payload: serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(handler.apply(&frame))
        .expect("show.load");
    let pending = state.audio_commands.lock().unwrap().clone();
    pending
}

#[test]
fn the_ceiling_is_the_shows_true_peak_from_show_load() {
    use nbe_engine::audio_control::{apply, AudioCommand};
    for (declared, expected, source) in [
        (Some(-3.0), -3.0, "show"),
        (None, DEFAULT_CEILING_DBTP, "default"),
    ] {
        let cmds = load_show(declared);
        let set: Vec<_> = cmds
            .iter()
            .filter(|c| matches!(c, AudioCommand::SetLimiterCeiling { .. }))
            .cloned()
            .collect();
        assert_eq!(
            set,
            vec![AudioCommand::SetLimiterCeiling {
                dbtp: expected,
                source
            }],
            "show.load sets the ceiling once, from the show ({declared:?})"
        );
        let mut g = AudioGraph::new(HOUSE_RATE);
        apply(&mut g, set, &std::collections::BTreeMap::new());
        assert_eq!(g.limiter_ceiling_dbtp(), expected);
        g.set_source(
            BusId::Music,
            vec![Source::Tone {
                hz: 997.0,
                amplitude: 2.0,
            }],
        );
        let out = render(&mut g, SAMPLE_RATE as usize / 2);
        let peak = sample_peak(&out);
        let ceiling = 10f64.powf(expected / 20.0);
        assert!(
            peak <= ceiling * (1.0 + 1e-6) && peak > ceiling * 0.9,
            "the {expected} dBTP ceiling is the one in force: the output peaks at {:.2} dBFS",
            db(peak)
        );
    }
}
