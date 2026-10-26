//! Device audio on hardware (the device-audio design record, D1, D2 and
//! §8.10): the normative machine's cases.
//!
//! These open real devices and, for the rate guard, switch the speakers' rate
//! and put it back. So they run only on the normative machine AND with
//! `NBE_DEVICE_TESTS=1` set; anywhere else each prints a `SKIP normative:`
//! line and passes (the dress's fail-closed rule: `CI` or `GITHUB_ACTIONS`
//! set is never the normative machine). The loopback cases use VB-Cable,
//! which is clocked from the host (0 ppm, design record §2.4): it proves the
//! tone, the latency compensation and the overload path, never drift. Drift
//! is the speakers' (+12.76 ppm at 44.1 kHz, §2.4).
//!
//! The one case that always runs, `observational_…`, lists what devices a
//! machine has and asserts nothing: CI's observational step reads it to
//! answer §5's unknown (does a `macos-14` runner have an output device?).
#![cfg(target_os = "macos")]

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use nbe_engine::audio::{CHANNELS, SAMPLE_RATE};
use nbe_engine::audio_control::AudioCommand;
use nbe_engine::audio_device::{DeviceChoice, DeviceSink};
use nbe_engine::audio_driver::{AudioDriver, DEVICE_POLL};
use nbe_engine::scene::ElementAudio;
use nbe_engine::state::EngineState;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HOUSE: u32 = 30;
const BLOCK: usize = 1600;
const HZ: f64 = 999.0;

/// `None` when this machine may run the device cases; otherwise why not.
fn device_tests_skip() -> Option<&'static str> {
    if std::env::var_os("CI").is_some() || std::env::var_os("GITHUB_ACTIONS").is_some() {
        return Some("not the normative machine (CI)");
    }
    if std::env::var("NBE_DEVICE_TESTS").as_deref() != Ok("1") {
        return Some("set NBE_DEVICE_TESTS=1 to open devices on this machine");
    }
    None
}

macro_rules! normative_only {
    ($what:expr) => {
        if let Some(why) = device_tests_skip() {
            println!("SKIP normative: {} ({why})", $what);
            return;
        }
    };
}

/// An exact 999 Hz sine at `amp`, `seconds` long: 999 whole cycles a second,
/// so it loops without a seam.
fn tone(amp: f32, seconds: usize) -> Arc<Vec<f32>> {
    let frames = SAMPLE_RATE as usize * seconds;
    Arc::new(
        (0..frames)
            .flat_map(|n| {
                let v = amp
                    * (2.0 * std::f64::consts::PI * HZ * n as f64 / SAMPLE_RATE as f64).sin()
                        as f32;
                [v; CHANNELS]
            })
            .collect(),
    )
}

/// An engine state with `samples` resident as asset "tone".
fn engine_with(samples: Arc<Vec<f32>>) -> Arc<EngineState> {
    let state = Arc::new(EngineState::new(HOUSE));
    state
        .audio_assets
        .lock()
        .unwrap()
        .insert("tone".into(), samples);
    state.package_generation.fetch_add(1, Ordering::SeqCst);
    state
}

fn take(state: &EngineState, t0: u64, looping: bool) {
    state
        .audio_commands
        .lock()
        .unwrap()
        .push(AudioCommand::TakeItem {
            item_ref: "A1".into(),
            sources: vec![ElementAudio {
                element_id: "main".into(),
                asset_id: "tone".into(),
                bus: "clip".into(),
                gain_db: 0.0,
                looping,
            }],
            t0,
            mode: "cut".into(),
            ramp_ms: 5.0,
            crossfade_frames: 0,
        });
}

/// The production device-mode cadence, on this thread, for `d`.
fn drive(driver: &mut AudioDriver, d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        driver.cycle_device();
        std::thread::sleep(DEVICE_POLL);
    }
}

/// VB-Cable's input, captured: the left channel, and per callback the frame
/// index it starts on and the instant its first frame was captured.
struct Capture {
    left: Arc<Mutex<Vec<f32>>>,
    marks: Arc<Mutex<Vec<(usize, Instant)>>>,
    _stream: cpal::Stream,
}

fn capture(needle: &str) -> Capture {
    let host = cpal::default_host();
    let device = host
        .input_devices()
        .expect("input devices")
        .find(|d| {
            d.description()
                .map(|x| x.name().contains(needle))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("no input device named like {needle}"));
    let left = Arc::new(Mutex::new(Vec::<f32>::with_capacity(
        SAMPLE_RATE as usize * 120,
    )));
    let marks = Arc::new(Mutex::new(Vec::<(usize, Instant)>::new()));
    let (l, m) = (left.clone(), marks.clone());
    let stream = device
        .build_input_stream::<f32, _, _>(
            cpal::StreamConfig {
                channels: CHANNELS as cpal::ChannelCount,
                sample_rate: SAMPLE_RATE,
                buffer_size: cpal::BufferSize::Default,
            },
            move |data: &[f32], info: &cpal::InputCallbackInfo| {
                let arrived = Instant::now();
                let ts = info.timestamp();
                let behind = ts.callback.saturating_duration_since(ts.capture);
                let mut l = l.lock().unwrap();
                m.lock().unwrap().push((l.len(), arrived - behind));
                l.extend(data.chunks(CHANNELS).map(|f| f[0]));
            },
            |e| eprintln!("capture error: {e}"),
            None,
        )
        .expect("the loopback input opens");
    stream.play().expect("the loopback input plays");
    Capture {
        left,
        marks,
        _stream: stream,
    }
}

impl Capture {
    /// The instant captured frame `i` was captured, from the callback marks.
    fn instant_of(&self, i: usize) -> Option<Instant> {
        let marks = self.marks.lock().unwrap();
        let (start, at) = *marks.iter().rev().find(|(s, _)| *s <= i)?;
        Some(at + Duration::from_secs_f64((i - start) as f64 / SAMPLE_RATE as f64))
    }
}

/// Goertzel power at `HZ` over `x`.
fn goertzel(x: &[f32]) -> f64 {
    let w = 2.0 * std::f64::consts::PI * HZ / SAMPLE_RATE as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s = v as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// The first 1 ms window from `from` whose 999 Hz power is 30 dB over the
/// median of the 200 windows before it: the onset, as a capture frame.
fn onset(x: &[f32], from: usize) -> Option<usize> {
    const W: usize = 48;
    let windows: Vec<f64> = x[from..].chunks(W).map(goertzel).collect();
    let mut floor: Vec<f64> = windows.iter().take(200).copied().collect();
    floor.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = floor
        .get(floor.len() / 2)
        .copied()
        .unwrap_or(0.0)
        .max(1e-20);
    windows
        .iter()
        .position(|&p| 10.0 * (p / median).log10() > 30.0)
        .map(|k| from + k * W)
}

/// Residual against the least-squares sine at `hz`, in dB re the tone.
fn residual_db(x: &[f32], hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / SAMPLE_RATE as f64;
    let (mut ss, mut sc, mut cc, mut xs, mut xc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (n, &v) in x.iter().enumerate() {
        let (s, c) = ((w * n as f64).sin(), (w * n as f64).cos());
        ss += s * s;
        sc += s * c;
        cc += c * c;
        xs += v as f64 * s;
        xc += v as f64 * c;
    }
    let det = ss * cc - sc * sc;
    let (a, b) = ((xs * cc - xc * sc) / det, (xc * ss - xs * sc) / det);
    let (mut t, mut r) = (0.0, 0.0);
    for (n, &v) in x.iter().enumerate() {
        let f = a * (w * n as f64).sin() + b * (w * n as f64).cos();
        t += f * f;
        r += (v as f64 - f) * (v as f64 - f);
    }
    10.0 * (r / t).log10()
}

// --------------------------------------------------------------- observational

/// What this machine has: CI's observational step prints it (§5's unknown).
/// Asserts nothing.
#[test]
fn observational_the_output_devices_this_machine_has() {
    let host = cpal::default_host();
    println!("DEVICES host {:?}", host.id());
    match host.output_devices() {
        Ok(devices) => {
            for d in devices {
                let name = d
                    .description()
                    .map(|x| x.name().to_string())
                    .unwrap_or_default();
                println!(
                    "DEVICES output {name:?}: default config {:?}",
                    d.default_output_config()
                );
            }
        }
        Err(e) => println!("DEVICES no output devices: {e}"),
    }
    match host.default_output_device() {
        Some(d) => println!(
            "DEVICES default output {:?}",
            d.description().map(|x| x.name().to_string())
        ),
        None => println!("DEVICES no default output device"),
    }
    match nbe_decode::hal::default_output_device() {
        Ok(id) => println!(
            "DEVICES HAL default {id}: nominal {:?} Hz, latency {:?}",
            nbe_decode::hal::nominal_rate(id),
            nbe_decode::hal::output_latency(id)
        ),
        Err(e) => println!("DEVICES HAL default: {e}"),
    }
}

// -------------------------------------------------------------- D1: the tone

/// VB-Cable's own output-to-input path, in ms: a raw cpal stream writes a
/// 999 Hz burst at a known frame, and its captured onset is read against the
/// instant cpal predicted for that frame. The HAL reports VB-Cable's latency
/// as 0, 0 and 0 (design record §2.1), so the path is measured, not read.
fn vb_cable_path_ms(cap: &Capture) -> f64 {
    const AT: u64 = 24_000; // half a second in
    let host = cpal::default_host();
    let device = host
        .output_devices()
        .unwrap()
        .find(|d| {
            d.description()
                .map(|x| x.name().contains("VB-Cable"))
                .unwrap_or(false)
        })
        .expect("VB-Cable output");
    let predicted: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let p = predicted.clone();
    let mut frame = 0u64;
    let stream = device
        .build_output_stream::<f32, _, _>(
            cpal::StreamConfig {
                channels: CHANNELS as cpal::ChannelCount,
                sample_rate: SAMPLE_RATE,
                buffer_size: cpal::BufferSize::Default,
            },
            move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let arrived = Instant::now();
                let ts = info.timestamp();
                let ahead = ts.playback.saturating_duration_since(ts.callback);
                let n = (out.len() / CHANNELS) as u64;
                if frame <= AT && AT < frame + n {
                    *p.lock().unwrap() = Some(
                        arrived
                            + ahead
                            + Duration::from_secs_f64((AT - frame) as f64 / SAMPLE_RATE as f64),
                    );
                }
                for (i, f) in out.chunks_mut(CHANNELS).enumerate() {
                    let k = frame + i as u64;
                    let v = if (AT..AT + 14_400).contains(&k) {
                        0.25 * (2.0 * std::f64::consts::PI * HZ * (k - AT) as f64
                            / SAMPLE_RATE as f64)
                            .sin() as f32
                    } else {
                        0.0
                    };
                    f.fill(v);
                }
                frame += n;
            },
            |e| eprintln!("calibration error: {e}"),
            None,
        )
        .expect("a raw VB-Cable output");
    stream.play().unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    drop(stream);
    let predicted = predicted
        .lock()
        .unwrap()
        .expect("the burst's frame was played");
    let left = cap.left.lock().unwrap().clone();
    let at = onset(&left, 0).expect("the calibration burst is captured");
    let heard = cap.instant_of(at).unwrap();
    heard.duration_since(predicted).as_secs_f64() * 1e3
}

/// D1's proving case: a file's audio, through the engine's device sink, out a
/// device and back in. The tone is taken at a master frame; with the latency
/// compensated, its onset reaches the loopback at that frame's master time
/// plus VB-Cable's own path, which is measured first.
#[test]
fn a_tone_is_heard_through_the_loopback_at_its_master_time() {
    normative_only!("the tone heard through the VB-Cable loopback (D1)");
    let cap = capture("VB-Cable");
    std::thread::sleep(Duration::from_millis(300));
    let path_ms = vb_cable_path_ms(&cap);
    let calibrated_to = cap.left.lock().unwrap().len();

    let state = engine_with(tone(0.25, 1));
    let sink = DeviceSink::open(&DeviceChoice::NameContains("VB-Cable".into()), BLOCK)
        .expect("VB-Cable opens");
    let marks = sink.marks();
    let mut driver = AudioDriver::new(state.clone(), Box::new(sink), HOUSE);
    state.clock.lock().unwrap().start();
    drive(&mut driver, Duration::from_millis(500));
    let t0 = state.master_frame().unwrap() + 6;
    take(&state, t0, false);
    drive(&mut driver, Duration::from_millis(1500));
    let epoch = state.clock.lock().unwrap().epoch().unwrap();
    let due = epoch + Duration::from_secs_f64(t0 as f64 / HOUSE as f64);
    let left = cap.left.lock().unwrap().clone();
    let at = onset(&left, calibrated_to).expect("the tone is heard in the loopback capture");
    let heard = cap.instant_of(at).expect("the onset has a capture instant");
    let offset_ms = if heard >= due {
        heard.duration_since(due).as_secs_f64() * 1e3
    } else {
        -due.duration_since(heard).as_secs_f64() * 1e3
    };
    let presented = f64::from_bits(state.audio_drift_ms_bits.load(Ordering::SeqCst));
    println!(
        "TONE VB-Cable path {path_ms:+.2} ms; the engine's onset {offset_ms:+.2} ms against its master time, so {:+.2} ms beyond the path; presented skew {presented:+.3} ms; callbacks {}, short {}, overloads {}",
        offset_ms - path_ms,
        marks.callbacks.load(Ordering::Relaxed),
        marks.short_callbacks.load(Ordering::Relaxed),
        marks.xruns.load(Ordering::Relaxed)
    );
    assert!(
        presented.abs() <= 1.0,
        "the presented skew holds ±1 ms: {presented:+.3}"
    );
    assert!(
        (offset_ms - path_ms).abs() <= 2.0,
        "beyond VB-Cable's own path, the onset lands within the 1 ms detection window and ±1 ms of its master time: {:+.2} ms",
        offset_ms - path_ms
    );
}

// ------------------------------------------------------- D1 a: the rate guard

/// D1 a: the sink switches a 44.1 kHz device to 48 kHz as its own act, and
/// restores the prior rate when it closes. M-rate leaves cpal's 48 kHz behind.
#[test]
fn the_speakers_rate_is_switched_and_restored() {
    normative_only!("the speakers' rate switched and restored (D1 a)");
    let id = nbe_decode::hal::default_output_device().expect("a default output");
    let before = nbe_decode::hal::nominal_rate(id).expect("its rate");
    let prior = if before == 48_000.0 {
        nbe_decode::hal::set_nominal_rate(id, 44_100.0).expect("44.1 kHz for the test");
        std::thread::sleep(Duration::from_millis(300));
        44_100.0
    } else {
        before
    };
    let state = engine_with(tone(0.0, 1));
    let sink = DeviceSink::open(&DeviceChoice::Default, BLOCK).expect("the default device opens");
    assert_eq!(
        sink.info().prior_rate_hz,
        Some(prior),
        "the sink recorded the prior rate"
    );
    assert_eq!(
        nbe_decode::hal::nominal_rate(id).unwrap(),
        48_000.0,
        "open at 48 kHz"
    );
    let mut driver = AudioDriver::new(state, Box::new(sink), HOUSE);
    drive(&mut driver, Duration::from_millis(300));
    drop(driver);
    let mut now = 0.0;
    for _ in 0..100 {
        now = nbe_decode::hal::nominal_rate(id).unwrap();
        if now == prior {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("RATE prior {prior} Hz, open 48000 Hz, after close {now} Hz");
    if before == 48_000.0 {
        let _ = nbe_decode::hal::set_nominal_rate(id, 48_000.0);
    }
    assert_eq!(now, prior, "closing restores the device's prior rate");
}

// --------------------------------------------------- §8.10: the forced stall

/// §8.10: a callback held 40 ms is an overload the HAL reports; it is one
/// underrun episode, counted and logged once, and the ring's fill keeps the
/// rest whole. M-xrun (the overload not counted) reads zero.
#[test]
fn a_forced_stall_is_one_counted_underrun_episode() {
    normative_only!("a forced stall, one underrun episode (§8.10)");
    let state = engine_with(tone(0.25, 1));
    let sink = DeviceSink::open(&DeviceChoice::NameContains("VB-Cable".into()), BLOCK)
        .expect("VB-Cable opens");
    let marks = sink.marks();
    let mut driver = AudioDriver::new(state.clone(), Box::new(sink), HOUSE);
    state.clock.lock().unwrap().start();
    take(&state, 0, true);
    drive(&mut driver, Duration::from_millis(500));
    let before = state.audio_underruns_total.load(Ordering::SeqCst);
    marks.stall_next_ms.store(40, Ordering::Relaxed);
    drive(&mut driver, Duration::from_millis(800));
    let after = state.audio_underruns_total.load(Ordering::SeqCst);
    println!(
        "STALL overloads {}, short callbacks {}, underruns {before} -> {after}, episodes {}",
        marks.xruns.load(Ordering::Relaxed),
        marks.short_callbacks.load(Ordering::Relaxed),
        driver.episodes
    );
    assert!(
        marks.xruns.load(Ordering::Relaxed) >= 1,
        "the HAL reported the overload"
    );
    assert!(after > before, "the overload is an underrun on the tick");
    assert_eq!(driver.episodes, 1, "one stall, one episode, logged once");
    assert!(
        !state.fallback_active(),
        "an audio fault never raises the slate"
    );
}

// ------------------------------------------- D2's addition, through the loopback

/// The slip residual on the device: VB-Cable is host-clocked, so 100 ppm is
/// injected into the planner to force corrections, and the looping 999 Hz
/// tone is captured back. Each 1600-frame window's residual against the tone
/// (frequency freed) bounds what a slip adds on the real path.
#[test]
fn the_slip_residual_through_the_loopback() {
    normative_only!("the slip residual through the VB-Cable loopback (D2)");
    let cap = capture("VB-Cable");
    std::thread::sleep(Duration::from_millis(300));
    let state = engine_with(tone(0.25, 1));
    let sink = DeviceSink::open(&DeviceChoice::NameContains("VB-Cable".into()), BLOCK)
        .expect("VB-Cable opens");
    let marks = sink.marks();
    let mut driver = AudioDriver::new(state.clone(), Box::new(sink), HOUSE);
    driver.feeder.inject_ppm = std::env::var("NBE_INJECT_PPM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100.0);
    state.clock.lock().unwrap().start();
    take(&state, 0, true);
    drive(&mut driver, Duration::from_secs(20));
    let slips = driver.feeder.drops + driver.feeder.duplicates;
    let short = marks.short_callbacks.load(Ordering::Relaxed);
    let overloads = marks.xruns.load(Ordering::Relaxed);
    // Close the device before the analysis: left open with nobody pushing,
    // its ring runs dry and every callback counts short (the first run of
    // this case counted 559 that way, all after the drive).
    let end = cap
        .left
        .lock()
        .unwrap()
        .len()
        .saturating_sub(SAMPLE_RATE as usize);
    drop(driver);
    let left = cap.left.lock().unwrap().clone();
    let start = onset(&left, 0).expect("the tone is captured") + SAMPLE_RATE as usize;
    let free = |x: &[f32]| {
        (-40..=40)
            .map(|k| residual_db(x, HZ * (1.0 + k as f64 * 2.5e-5)))
            .fold(f64::INFINITY, f64::min)
    };
    let windows: Vec<f64> = left[start..end]
        .as_chunks::<BLOCK>()
        .0
        .iter()
        .map(|w| free(w))
        .collect();
    let bad: Vec<(usize, f64)> = windows
        .iter()
        .enumerate()
        .filter(|(_, r)| **r > -80.0)
        .map(|(i, r)| (start + i * BLOCK, *r))
        .collect();
    // A gap in the capture (an input overrun) shows as a callback whose
    // capture instant is not where the frames before it put it.
    let gaps: Vec<(usize, f64)> = {
        let m = cap.marks.lock().unwrap();
        m.windows(2)
            .filter_map(|w| {
                let expect = (w[1].0 - w[0].0) as f64 / SAMPLE_RATE as f64;
                let got = w[1].1.saturating_duration_since(w[0].1).as_secs_f64();
                ((got - expect).abs() > 0.002).then_some((w[1].0, (got - expect) * 1e3))
            })
            .collect()
    };
    let mut rs = windows.clone();
    rs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (median, worst) = (rs[rs.len() / 2], *rs.last().unwrap());
    println!(
        "LOOPBACK-RESIDUAL slips {slips}, windows {}, median {median:.1} dB, worst {worst:.1} dB; windows over -80 dB {} (positions {:?}); capture gaps {:?}; device short {short} overloads {overloads}",
        rs.len(),
        bad.len(),
        bad.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        gaps,
    );
    assert!(slips > 50, "the injected ppm forced corrections: {slips}");
    assert_eq!((short, overloads), (0, 0), "the device path ran whole");
    assert!(gaps.is_empty(), "the capture ran whole");
    assert!(
        median < -120.0,
        "away from a slip, the device path is the tone: {median:.1} dB"
    );
    // A 1600-frame window not aligned to the blocks straddles a re-spaced
    // block and its neighbour, so a slip shows in at most two windows, and
    // reads as its own re-timing, which the copy-out bounds (fixed
    // frequency, under -25 dB).
    assert!(
        bad.len() as u64 <= 2 * slips + 2,
        "every impaired window is a slip's: {} windows for {slips} slips",
        bad.len()
    );
    assert!(
        worst < -25.0,
        "no window worse than a slip's own re-timing: {worst:.1} dB"
    );
}

// ------------------------------------------ D2's proving case: 30 minutes

/// D2's proving case (the user's specification): the skew at 1, 10 and 30
/// minutes, both ways, each inside ±1 ms.
/// - **Engine-internal, on the speakers** (the real crystal, +12.76 ppm at
///   44.1 kHz; 48 kHz measured here for the first time): the presented skew
///   the device's own marks read back.
/// - **End to end, through the VB-Cable loopback**: a burst of the tone every
///   10 s, its captured onset against its master time.
///
/// Ignored: run explicitly, `NBE_DEVICE_TESTS=1 cargo test --release -p
/// nbe-engine --test device_audio_hw -- --ignored --nocapture`.
#[test]
#[ignore = "30 minutes on the normative machine's devices"]
fn the_skew_holds_for_thirty_minutes_on_the_speakers_and_through_the_loopback() {
    normative_only!("the 30-minute skew (D2's proving case)");
    let minutes: u64 = std::env::var("NBE_SKEW_MINUTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let checkpoints = [1u64, 10, 30].map(|m| m.min(minutes));
    let speakers = std::thread::spawn(move || {
        let state = engine_with(tone(0.0, 1));
        let sink = DeviceSink::open(&DeviceChoice::Default, BLOCK).expect("the speakers open");
        let name = sink.info().name.clone();
        let mut driver = AudioDriver::new(state.clone(), Box::new(sink), HOUSE);
        state.clock.lock().unwrap().start();
        let started = Instant::now();
        let mut max_abs = 0.0f64;
        let mut at = Vec::new();
        while started.elapsed() < Duration::from_secs(minutes * 60) {
            drive(&mut driver, Duration::from_millis(500));
            let skew = f64::from_bits(state.audio_drift_ms_bits.load(Ordering::SeqCst));
            if started.elapsed() > Duration::from_secs(2) {
                max_abs = max_abs.max(skew.abs());
            }
            for &m in &checkpoints {
                if at.iter().all(|(x, _)| *x != m)
                    && started.elapsed() >= Duration::from_secs(m * 60)
                {
                    at.push((m, skew));
                }
            }
        }
        (
            name,
            max_abs,
            at,
            driver.feeder.drops,
            driver.feeder.duplicates,
            driver.feeder.realigns,
        )
    });
    let state = engine_with(tone(0.25, 1));
    let sink = DeviceSink::open(&DeviceChoice::NameContains("VB-Cable".into()), BLOCK)
        .expect("VB-Cable opens");
    let cap = capture("VB-Cable");
    let mut driver = AudioDriver::new(state.clone(), Box::new(sink), HOUSE);
    state.clock.lock().unwrap().start();
    let started = Instant::now();
    let mut bursts: Vec<(u64, u64)> = Vec::new(); // (minute mark, t0)
    let mut next = 2u64;
    while started.elapsed() < Duration::from_secs(minutes * 60 + 5) {
        drive(&mut driver, Duration::from_millis(250));
        if started.elapsed() >= Duration::from_secs(next)
            && started.elapsed() < Duration::from_secs(minutes * 60)
        {
            let t0 = state.master_frame().unwrap() + 6;
            take(&state, t0, false);
            bursts.push((next, t0));
            next += 10;
        }
    }
    let epoch = state.clock.lock().unwrap().epoch().unwrap();
    let left = cap.left.lock().unwrap().clone();
    let mut e2e: Vec<(u64, f64)> = Vec::new();
    for &(s, t0) in &bursts {
        let due = epoch + Duration::from_secs_f64(t0 as f64 / HOUSE as f64);
        let from = cap
            .marks
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(_, at)| *at + Duration::from_millis(300) <= due)
            .map(|(i, _)| *i)
            .unwrap_or(0);
        if let Some(i) = onset(&left, from) {
            if let Some(heard) = cap.instant_of(i) {
                let ms = if heard >= due {
                    heard.duration_since(due).as_secs_f64() * 1e3
                } else {
                    -due.duration_since(heard).as_secs_f64() * 1e3
                };
                e2e.push((s, ms));
            }
        }
    }
    let (name, max_abs, at, drops, dups, realigns) = speakers.join().expect("the speakers' thread");
    let e2e_at = |m: u64| {
        e2e.iter()
            .min_by_key(|(s, _)| (*s as i64 - (m * 60) as i64).abs())
            .map(|(_, ms)| *ms)
    };
    for &m in &checkpoints {
        println!(
            "SKEW {m} min: {name} presented {:?} ms; loopback onset {:?} ms",
            at.iter().find(|(x, _)| *x == m).map(|(_, s)| *s),
            e2e_at(m)
        );
    }
    let spread = e2e
        .iter()
        .map(|(_, ms)| *ms)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    println!(
        "SKEW summary: {name} max |presented| {max_abs:.3} ms (drops {drops}, duplicates {dups}, realigns {realigns}); loopback {} bursts, onsets {:.2}..{:.2} ms",
        e2e.len(),
        spread.0,
        spread.1
    );
    assert!(
        max_abs <= 1.0,
        "the speakers' presented skew left ±1 ms: {max_abs:.3}"
    );
    assert!(
        spread.1 - spread.0 <= 2.0,
        "the loopback onsets hold within the 1 ms windows' reach of each other: {:.2}..{:.2}",
        spread.0,
        spread.1
    );
}
