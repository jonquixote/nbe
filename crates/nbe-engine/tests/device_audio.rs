//! Device audio, measured without a device (the device-audio design record,
//! D1, D2 and D7: the CI surface).
//!
//! What CI can honestly exercise, unit level: the ring, the copy-out the
//! device callback runs, the clock's slip against a simulated device crystal
//! over a simulated 30 minutes, the device-referenced drift the tick
//! publishes, the rate guard's decision table, the sink choice and its
//! fallback. Opening a real device, the tone heard, the overload path and the
//! 30-minute skew on hardware are the normative machine's (`device_audio_hw`).
//!
//! Each test names the production path it covers and fails when that path is
//! removed (Standards §2a).

use nbe_engine::audio::{CHANNELS, SAMPLE_RATE};
use nbe_engine::audio_clock::{copy_out, respace, DeviceMarks, Feeder, Mark, SLIP_THRESHOLD_MS};
use nbe_engine::audio_driver::{
    open_sink, rate_plan, sink_choice, AudioDriver, AudioSink, DeviceInfo, DeviceIo, RatePlan,
    SinkChoice,
};
use nbe_engine::audio_ring::{ring, RingConsumer};
use nbe_engine::state::EngineState;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[allow(unsafe_code)]
mod alloc_probe {
    //! A counting global allocator for the callback's §8.9 gate:
    //! `tests/prompt06.rs`'s, thread-local for the same reason (the suite
    //! runs in parallel). The allocator never ships, and the CI gate that
    //! confines `unsafe` scopes itself to `src/`.
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ALLOCS: Cell<usize> = const { Cell::new(0) };
        static ARMED: Cell<bool> = const { Cell::new(false) };
    }

    pub struct CountingAlloc;

    unsafe impl GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            note();
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            note();
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    fn note() {
        let _ = ARMED.try_with(|armed| {
            if armed.get() {
                let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
            }
        });
    }

    pub fn allocations_during(f: impl FnOnce()) -> usize {
        ALLOCS.with(|n| n.set(0));
        ARMED.with(|a| a.set(true));
        f();
        ARMED.with(|a| a.set(false));
        ALLOCS.with(|n| n.get())
    }
}

use alloc_probe::{allocations_during, CountingAlloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const BLOCK: usize = 1600;

fn frames(n: usize, first: f32) -> Vec<f32> {
    (0..n * CHANNELS).map(|i| first + i as f32).collect()
}

// ------------------------------------------------------------------ the ring

#[test]
fn the_ring_copies_out_in_order_across_its_wrap() {
    let (mut p, mut c) = ring(4);
    let mut out = vec![0.0; 3 * CHANNELS];
    for round in 0..5 {
        let block = frames(3, round as f32 * 100.0);
        assert_eq!(
            p.push(&block),
            block.len(),
            "round {round}: three frames fit"
        );
        assert_eq!(c.pop_into(&mut out), out.len());
        assert_eq!(out, block, "round {round}: out in the order in");
    }
    assert_eq!(c.read_frames(), 15);
    assert_eq!(p.written_frames(), 15);
}

#[test]
fn the_ring_never_overfills_and_takes_whole_frames() {
    let (mut p, _c) = ring(4);
    assert_eq!(
        p.push(&frames(6, 0.0)),
        4 * CHANNELS,
        "only the capacity goes in"
    );
    assert_eq!(p.fill_frames(), 4);
    assert_eq!(p.push(&frames(1, 0.0)), 0, "a full ring takes nothing");
}

/// M-empty: an empty ring is silence and a counted underrun, never the stale
/// samples of the slot's last lap.
#[test]
fn a_short_callback_plays_silence_and_counts_one_underrun() {
    let (mut p, mut c) = ring(8);
    let marks = DeviceMarks::new();
    // Fill a lap with loud samples and drain it, so every slot holds a stale value.
    p.push(&[0.9; 8 * CHANNELS]);
    let mut out = vec![0.0; 8 * CHANNELS];
    copy_out(&mut c, &marks, &mut out, 0);
    p.push(&[0.5; 2 * CHANNELS]);
    let mut out = vec![7.0; 6 * CHANNELS];
    copy_out(&mut c, &marks, &mut out, 1_000);
    assert_eq!(
        &out[..2 * CHANNELS],
        &[0.5; 2 * CHANNELS][..],
        "the two frames the ring held"
    );
    assert!(
        out[2 * CHANNELS..].iter().all(|&v| v == 0.0),
        "the rest is silence, not the ring's stale lap: {:?}",
        &out[2 * CHANNELS..]
    );
    assert_eq!(
        marks.short_callbacks.load(Ordering::Relaxed),
        1,
        "one short callback, one underrun"
    );
    assert_eq!(marks.short_frames.load(Ordering::Relaxed), 4);
    assert_eq!(
        marks.latest(),
        Some(Mark {
            frame: 8,
            ns: 1_000
        }),
        "the mark names the ring frame the buffer starts on"
    );
}

/// M-ring: the callback's body copies out and marks, and allocates nothing
/// (SPEC §8.9). A callback that rendered the graph, or logged, would allocate.
#[test]
fn the_callback_copy_out_allocates_nothing() {
    let (mut p, mut c) = ring(BLOCK * 4);
    let marks = DeviceMarks::new();
    let block = vec![0.25f32; BLOCK * CHANNELS];
    let mut out = vec![0.0f32; 512 * CHANNELS];
    let mut short = vec![0.0f32; 512 * CHANNELS];
    let (_p2, mut c2) = ring(16);
    p.push(&block);
    let n = allocations_during(|| {
        for i in 0..3 {
            copy_out(&mut c, &marks, &mut out, i * 10_000);
        }
        // and the short path
        copy_out(&mut c2, &marks, &mut short, 99);
    });
    assert_eq!(n, 0, "the device callback must not allocate (SPEC §8.9)");
    assert_eq!(marks.callbacks.load(Ordering::Relaxed), 4);
}

// ------------------------------------------------------------- re-spacing

#[test]
fn a_block_that_does_not_slip_passes_through_bit_for_bit() {
    let input = frames(BLOCK, 0.125);
    let mut out = vec![0.0; BLOCK * CHANNELS];
    respace(&input, [9.0, 9.0], &mut out);
    assert_eq!(out, input);
}

/// The slip's re-spacing: a 999 Hz tone spread one sample longer or shorter
/// keeps its ends exactly and follows the ideal re-timed sine closely.
#[test]
fn a_slip_respaces_end_to_end_without_a_step() {
    let hz = 999.0f64;
    let sine = |n: f64| (2.0 * std::f64::consts::PI * hz * n / SAMPLE_RATE as f64).sin() as f32;
    let base = 48_000.0 * 3.0;
    let input: Vec<f32> = (0..BLOCK)
        .flat_map(|i| [sine(base + i as f64); CHANNELS])
        .collect();
    let prev = [sine(base - 1.0); CHANNELS];
    for n_out in [BLOCK - 1, BLOCK + 1] {
        let mut out = vec![0.0; n_out * CHANNELS];
        respace(&input, prev, &mut out);
        assert_eq!(
            out[0], input[0],
            "{n_out}: the first frame lands on the first"
        );
        assert_eq!(
            out[(n_out - 1) * CHANNELS],
            input[(BLOCK - 1) * CHANNELS],
            "{n_out}: the last frame lands on the last"
        );
        let step = (BLOCK - 1) as f64 / (n_out - 1) as f64;
        let worst = (0..n_out)
            .map(|j| (out[j * CHANNELS] - sine(base + j as f64 * step)).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 1e-3,
            "{n_out}: the re-spaced block follows the ideal re-timed sine: worst error {worst}"
        );
    }
}

// --------------------------------------------------------------- the clock

struct Sim {
    max_abs_skew_ms: f64,
    skew_at_min: Vec<(u32, f64)>,
    drops: u64,
    duplicates: u64,
    realigns: u64,
    short: u64,
}

/// A simulated device against the production `Feeder`: callbacks of 512 frames
/// at the device's own rate, `48000 × (1 + ppm)` frames a host second, each
/// heard a fixed 18.4 ms after it arrives (the speakers' HAL latency, design
/// record §2.1). The driver tops the ring up to its target before every
/// callback, as its 4 ms poll does. The skew is read back from the push log
/// at every mark, as the tick reads it.
fn simulate(ppm: f64, minutes: f64, slip_gain: i32) -> Sim {
    const BUF: u64 = 512;
    const LATENCY_NS: f64 = 18.4e6;
    let mut feeder = Feeder::new(BLOCK);
    feeder.slip_gain = slip_gain;
    let rendered = vec![0.0f32; BLOCK * CHANNELS];
    let device_hz = SAMPLE_RATE as f64 * (1.0 + ppm * 1e-6);
    let target = 2 * BLOCK as u64;
    let callbacks = (minutes * 60.0 * device_hz / BUF as f64) as u64;
    let (mut written, mut read) = (0u64, 0u64);
    let mut mark: Option<Mark> = None;
    let mut sim = Sim {
        max_abs_skew_ms: 0.0,
        skew_at_min: Vec::new(),
        drops: 0,
        duplicates: 0,
        realigns: 0,
        short: 0,
    };
    let mut next_minute = 1u32;
    for k in 0..callbacks {
        let t_ns = k as f64 * BUF as f64 / device_hz * 1e9;
        while written - read < target {
            let plan = feeder.plan(Some(0), written, mark, t_ns as u64, (written - read) + 883);
            written += (feeder.emit(&rendered, &plan, written).len() / CHANNELS) as u64;
        }
        let m = Mark {
            frame: read,
            ns: (t_ns + LATENCY_NS) as u64,
        };
        mark = Some(m);
        let take = BUF.min(written - read);
        if take < BUF {
            sim.short += 1;
        }
        read += take;
        if let Some(skew) = feeder.presented_skew_ms(m) {
            if t_ns > 1e9 {
                sim.max_abs_skew_ms = sim.max_abs_skew_ms.max(skew.abs());
            }
            if t_ns >= next_minute as f64 * 60e9 {
                sim.skew_at_min.push((next_minute, skew));
                next_minute += 1;
            }
        }
    }
    sim.drops = feeder.drops;
    sim.duplicates = feeder.duplicates;
    sim.realigns = feeder.realigns;
    sim
}

/// D2's guarantee: the device-referenced skew held within ±1 ms of the master
/// clock for 30 minutes, at the normative machine's measured +12.76 ppm, its
/// mirror, and a crystal eight times worse. M-slip-off and M-slip-sign fail it
/// (the next test is their control).
#[test]
fn the_slip_holds_the_skew_inside_one_ms_for_thirty_minutes() {
    for ppm in [12.76, -12.76, 100.0, -100.0] {
        let sim = simulate(ppm, 30.0, 1);
        let at = |m: u32| {
            sim.skew_at_min
                .iter()
                .find(|(x, _)| *x == m)
                .map(|(_, s)| *s)
        };
        println!(
            "DRIFT-SIM ppm={ppm:+} max|skew|={:.3} ms at1={:?} at10={:?} at30={:?} drops={} duplicates={} realigns={} short={}",
            sim.max_abs_skew_ms,
            at(1),
            at(10),
            at(29),
            sim.drops,
            sim.duplicates,
            sim.realigns,
            sim.short
        );
        assert!(
            sim.max_abs_skew_ms <= 1.0,
            "{ppm:+} ppm: the skew left ±1 ms ({:.3} ms)",
            sim.max_abs_skew_ms
        );
        assert_eq!(
            sim.realigns, 0,
            "{ppm:+} ppm: no realign on a steady crystal"
        );
        assert_eq!(sim.short, 0, "{ppm:+} ppm: the ring never ran short");
        let slips = sim.drops + sim.duplicates;
        assert!(slips > 0, "{ppm:+} ppm: a drifting crystal needs slips");
        // A fast device consumes ahead of the master: duplicates hold it back.
        if ppm > 0.0 {
            assert!(sim.duplicates > sim.drops, "{ppm:+} ppm: duplicates");
        } else {
            assert!(sim.drops > sim.duplicates, "{ppm:+} ppm: drops");
        }
    }
}

/// The control: the drift is real. Without the slip, or with its sign
/// reversed, the skew leaves ±1 ms within minutes, and only the realign's
/// jumps (each an audible splice) keep it bounded at all.
#[test]
fn without_the_slip_the_skew_leaves_one_ms() {
    for gain in [0, -1] {
        let sim = simulate(12.76, 30.0, gain);
        println!(
            "DRIFT-SIM-CONTROL gain={gain} max|skew|={:.3} ms realigns={}",
            sim.max_abs_skew_ms, sim.realigns
        );
        assert!(
            sim.max_abs_skew_ms > 1.0,
            "gain {gain}: the uncorrected skew must leave ±1 ms ({:.3})",
            sim.max_abs_skew_ms
        );
        assert!(sim.realigns > 0, "gain {gain}: only realigns bound it");
    }
}

#[test]
fn the_slip_threshold_sits_inside_the_guarantee() {
    assert!(SLIP_THRESHOLD_MS + 1000.0 / (SAMPLE_RATE as f64) < 1.0);
}

// ------------------------------------------------------- the rate guard (D1 a)

#[test]
fn the_rate_guard_keeps_switches_or_refuses() {
    assert_eq!(rate_plan(48_000.0, true), RatePlan::Keep);
    assert_eq!(rate_plan(48_000.0, false), RatePlan::Keep);
    assert_eq!(
        rate_plan(44_100.0, true),
        RatePlan::Switch { prior_hz: 44_100.0 }
    );
    assert_eq!(
        rate_plan(96_000.0, true),
        RatePlan::Switch { prior_hz: 96_000.0 }
    );
    assert_eq!(rate_plan(44_100.0, false), RatePlan::Refuse);
}

// ------------------------------------------------------ the sink choice (D7)

#[test]
fn the_sink_choice_reads_nbe_audio_sink() {
    assert_eq!(sink_choice(None), SinkChoice::Device);
    assert_eq!(sink_choice(Some("device")), SinkChoice::Device);
    assert_eq!(sink_choice(Some("")), SinkChoice::Device);
    assert_eq!(sink_choice(Some("null")), SinkChoice::Null);
    assert_eq!(sink_choice(Some(" null ")), SinkChoice::Null);
    assert_eq!(
        sink_choice(Some("speakers")),
        SinkChoice::Unknown("speakers".into())
    );
}

/// M-fallback: a sink that cannot be what was asked for is silence with its
/// reason on the tick, never a panic.
#[test]
fn a_sink_that_cannot_open_is_silence_with_its_reason() {
    let state = EngineState::new(30);
    let mut sink = open_sink(&SinkChoice::Unknown("speakers".into()), BLOCK, &state);
    assert!(sink.device().is_none(), "the fallback is the null sink");
    assert!(sink.write(&vec![0.0; BLOCK * CHANNELS]));
    let line = state.audio_device.lock().unwrap().clone();
    assert_eq!(line.state, "absent");
    assert!(line.reason.unwrap().contains("speakers"));

    let state = EngineState::new(30);
    let sink = open_sink(&SinkChoice::Null, BLOCK, &state);
    drop(sink);
    let line = state.audio_device.lock().unwrap().clone();
    assert_eq!(
        line.state, "null",
        "a configured null sink is not an absent device"
    );
}

// ------------------------------------------- the driver, with a fake device

struct FakeDevice {
    io: DeviceIo,
}

impl AudioSink for FakeDevice {
    fn write(&mut self, block: &[f32]) -> bool {
        self.io.producer.push(block) == block.len()
    }
    fn block_frames(&self) -> usize {
        BLOCK
    }
    fn device(&mut self) -> Option<&mut DeviceIo> {
        Some(&mut self.io)
    }
}

fn fake_device() -> (Box<FakeDevice>, RingConsumer, Arc<DeviceMarks>) {
    let (producer, consumer) = ring(BLOCK * 8);
    let marks = DeviceMarks::new();
    let sink = Box::new(FakeDevice {
        io: DeviceIo {
            producer,
            marks: marks.clone(),
            info: DeviceInfo {
                name: "fake".into(),
                rate_hz: SAMPLE_RATE,
                prior_rate_hz: None,
                latency_frames: 883,
            },
        },
    });
    (sink, consumer, marks)
}

/// §8.10: the device's short callbacks and its overloads are underruns, on
/// the tick, never a video fault. M-xrun removes the overload count.
#[test]
fn device_short_callbacks_and_overloads_are_underruns() {
    let state = Arc::new(EngineState::new(30));
    let (sink, mut consumer, marks) = fake_device();
    let mut driver = AudioDriver::new(state.clone(), sink, 30);
    assert!(driver.is_device());
    assert!(
        driver.cycle_device() >= 2,
        "the driver fills the ring to its target"
    );
    // Drain more than the ring holds: one short callback.
    let mut out = vec![0.0; BLOCK * 8 * CHANNELS];
    copy_out(&mut consumer, &marks, &mut out, 0);
    marks.xruns.fetch_add(2, Ordering::Relaxed);
    driver.cycle_device();
    assert_eq!(
        state.audio_underruns_total.load(Ordering::SeqCst),
        3,
        "one short callback and two overloads"
    );
    assert!(
        !state.fallback_active(),
        "an audio fault never raises the slate (§8.10)"
    );
}

/// R27: with a device open, `audioDriftMs` is the device's presented skew
/// (device-consumed against master-implied, SPEC §8.9), not the render
/// cadence's. A mark heard 3 ms later moves it by 3 ms.
#[test]
fn the_tick_drift_is_the_devices_presented_skew() {
    let state = Arc::new(EngineState::new(30));
    state.clock.lock().unwrap().start();
    let (sink, mut consumer, marks) = fake_device();
    let mut driver = AudioDriver::new(state.clone(), sink, 30);
    driver.cycle_device();
    let mut out = vec![0.0; 512 * CHANNELS];
    let heard = marks.ns_of(std::time::Instant::now()) + 40_000_000;
    copy_out(&mut consumer, &marks, &mut out, heard);
    driver.cycle_device();
    let first = f64::from_bits(state.audio_drift_ms_bits.load(Ordering::SeqCst));
    copy_out(
        &mut consumer,
        &marks,
        &mut out,
        heard + 512 * 1_000_000_000 / 48_000 + 3_000_000,
    );
    driver.cycle_device();
    let second = f64::from_bits(state.audio_drift_ms_bits.load(Ordering::SeqCst));
    assert!(
        (second - first - 3.0).abs() < 0.05,
        "a mark heard 3 ms later reads 3 ms later: {first:.3} then {second:.3}"
    );
}

/// §8.10's device loss: the engine continues silent on the null cadence, the
/// tick reads `lost`, and nothing blocks (U5's reopen is unbuilt).
#[test]
fn a_lost_device_falls_back_to_silence_and_says_so() {
    let state = Arc::new(EngineState::new(30));
    let (sink, _consumer, marks) = fake_device();
    let mut driver = AudioDriver::new(state.clone(), sink, 30);
    driver.cycle_device();
    marks.lost.store(true, Ordering::Relaxed);
    driver.cycle_device();
    assert!(
        !driver.is_device(),
        "the lost device is dropped for the null sink"
    );
    let line = state.audio_device.lock().unwrap().clone();
    assert_eq!(line.state, "lost");
    assert_eq!(line.name.as_deref(), Some("fake"));
    assert_eq!(driver.cycle(0), BLOCK, "the null cadence renders on");
}

/// D5's amendment: the ceiling in force reaches the tick, value and source.
#[test]
fn the_limiter_ceiling_in_force_reaches_the_tick() {
    let state = Arc::new(EngineState::new(30));
    let before = state.audio_limiter_ceiling.lock().unwrap().clone();
    assert_eq!(before.dbtp, nbe_engine::limiter::DEFAULT_CEILING_DBTP);
    assert_eq!(before.source, "default");
    assert_eq!(
        before,
        nbe_protocol::LimiterCeilingLine::default_ceiling(),
        "the wire's stub is the graph's default"
    );
    state.audio_commands.lock().unwrap().push(
        nbe_engine::audio_control::AudioCommand::SetLimiterCeiling {
            dbtp: -2.0,
            source: "show",
        },
    );
    let mut driver = AudioDriver::new(
        state.clone(),
        Box::new(nbe_engine::audio_driver::NullSink::new(BLOCK)),
        30,
    );
    driver.cycle(0);
    let after = state.audio_limiter_ceiling.lock().unwrap().clone();
    assert_eq!((after.dbtp, after.source.as_str()), (-2.0, "show"));
    let tick = serde_json::to_value(nbe_engine::telemetry::build_tick(&state)).unwrap();
    assert_eq!(tick["audioLimiterCeiling"]["dbtp"], -2.0);
    assert_eq!(tick["audioLimiterCeiling"]["source"], "show");
    assert_eq!(
        tick["audioDevice"]["state"], "none",
        "no driver reported: the stub"
    );
}

// ------------------------------------------- the slip residual (D2's addition)

/// Residual of `x` against the least-squares sine at `hz` (amplitude and
/// phase fitted), in dB relative to the fitted tone.
fn residual_db(x: &[f64], hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / SAMPLE_RATE as f64;
    let (mut ss, mut sc, mut cc, mut xs, mut xc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (n, &v) in x.iter().enumerate() {
        let (s, c) = ((w * n as f64).sin(), (w * n as f64).cos());
        ss += s * s;
        sc += s * c;
        cc += c * c;
        xs += v * s;
        xc += v * c;
    }
    let det = ss * cc - sc * sc;
    let (a, b) = ((xs * cc - xc * sc) / det, (xc * ss - xs * sc) / det);
    let (mut tone, mut resid) = (0.0, 0.0);
    for (n, &v) in x.iter().enumerate() {
        let fit = a * (w * n as f64).sin() + b * (w * n as f64).cos();
        tone += fit * fit;
        resid += (v - fit) * (v - fit);
    }
    10.0 * (resid / tone).log10()
}

/// The best residual over frequencies near `hz`: the fit's frequency freed,
/// so what remains is the interpolation's own distortion, not the slip's
/// intended 1/1600 re-timing.
fn free_residual_db(x: &[f64], hz: f64) -> f64 {
    (-40..=40)
        .map(|k| residual_db(x, hz * (1.0 + k as f64 * 2.5e-5)))
        .fold(f64::INFINITY, f64::min)
}

/// D2's addition (the user's ruling of 2026-10-21): the proving case carries
/// a correction-audibility measure. The 999 Hz tone (`av_tone.mp4`'s
/// frequency) at -12 dBFS goes through the production `Feeder` against a
/// simulated device at +100 ppm, so slips are forced every ~0.2 s, and the
/// stream it pushes is measured at the copy-out, where it is deterministic:
/// - **the slip residual**, per re-spaced block, against a 999 Hz fit with
///   the frequency fixed (the ruling's quantity: it includes the slip's own
///   0.06 % re-timing) and with the frequency freed (the interpolation's
///   distortion alone);
/// - **the floor**, the same measures over blocks that did not slip;
/// - **the join**: the largest sample step at any block boundary against the
///   tone's own largest step.
///
/// The bounds are the build's proposal, set from this measurement.
#[test]
fn the_slip_residual_is_measured_and_bounded() {
    const HZ: f64 = 999.0;
    let amp = 0.25f64;
    let mut feeder = Feeder::new(BLOCK);
    let device_hz = SAMPLE_RATE as f64 * (1.0 + 100e-6);
    let (mut written, mut read) = (0u64, 0u64);
    let mut mark: Option<Mark> = None;
    let mut stream: Vec<f64> = Vec::new();
    let mut blocks: Vec<(usize, usize, bool)> = Vec::new(); // (start, len, slipped)
    let mut rendered = vec![0.0f32; BLOCK * CHANNELS];
    for k in 0..(20.0 * device_hz / 512.0) as u64 {
        let t_ns = k as f64 * 512.0 / device_hz * 1e9;
        while written - read < 2 * BLOCK as u64 {
            let plan = feeder.plan(Some(0), written, mark, t_ns as u64, (written - read) + 883);
            for (f, frame) in rendered.chunks_mut(CHANNELS).enumerate() {
                let n = plan.base_sample + f as i64;
                let v =
                    amp * (2.0 * std::f64::consts::PI * HZ * n as f64 / SAMPLE_RATE as f64).sin();
                frame.fill(v as f32);
            }
            let out = feeder.emit(&rendered, &plan, written);
            let len = out.len() / CHANNELS;
            blocks.push((stream.len(), len, len != BLOCK));
            stream.extend(out.chunks(CHANNELS).map(|f| f[0] as f64));
            written += len as u64;
        }
        mark = Some(Mark {
            frame: read,
            ns: (t_ns + 18.4e6) as u64,
        });
        read += 512;
    }
    let (mut slip_fixed, mut slip_free, mut floor_fixed): (f64, f64, f64) =
        (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut slips = 0;
    for &(start, len, slipped) in blocks.iter().skip(2) {
        let x = &stream[start..start + len];
        if slipped {
            slips += 1;
            slip_fixed = slip_fixed.max(residual_db(x, HZ));
            slip_free = slip_free.max(free_residual_db(x, HZ));
        } else {
            floor_fixed = floor_fixed.max(residual_db(x, HZ));
        }
    }
    let tone_step = 2.0 * std::f64::consts::PI * HZ / SAMPLE_RATE as f64 * amp;
    let worst_join = blocks
        .iter()
        .skip(2)
        .map(|&(start, _, _)| (stream[start] - stream[start - 1]).abs())
        .fold(0.0, f64::max);
    println!(
        "SLIP-RESIDUAL slips={slips} fixed={slip_fixed:.1} dB free={slip_free:.1} dB floor={floor_fixed:.1} dB join={:.3}x the tone's largest step",
        worst_join / tone_step
    );
    assert!(slips > 50, "+100 ppm over 20 s forces slips: {slips}");
    assert!(
        floor_fixed < -100.0,
        "an unaltered block is the tone: {floor_fixed:.1} dB"
    );
    assert!(
        slip_free < -80.0,
        "the cubic re-spacing's own distortion: {slip_free:.1} dB"
    );
    assert!(
        slip_fixed < -25.0,
        "a slipped block against the fixed 999 Hz: the 1/1600 re-timing and nothing worse: {slip_fixed:.1} dB"
    );
    assert!(
        worst_join <= tone_step * 1.01,
        "no join steps further than the tone itself does: {:.3}x",
        worst_join / tone_step
    );
}

/// The device opens before the audio thread's first block. Its callbacks then
/// play silence, and that is a stream not yet started, not an underrun: a
/// false count at every boot was found on the normative machine (16 at one
/// open). After the first push, a short callback counts.
#[test]
fn silence_before_the_first_block_is_not_an_underrun() {
    let (mut p, mut c) = ring(8);
    let marks = DeviceMarks::new();
    let mut out = vec![1.0; 4 * CHANNELS];
    copy_out(&mut c, &marks, &mut out, 0);
    assert!(out.iter().all(|&v| v == 0.0), "silence");
    assert_eq!(
        marks.short_callbacks.load(Ordering::Relaxed),
        0,
        "not an underrun"
    );
    p.push(&[0.5; 2 * CHANNELS]);
    copy_out(&mut c, &marks, &mut out, 1);
    assert_eq!(
        marks.short_callbacks.load(Ordering::Relaxed),
        1,
        "after the first block, a short callback is one"
    );
}

/// A sink that keeps every block it is given.
struct KeepSink(Arc<std::sync::Mutex<Vec<f32>>>);

impl AudioSink for KeepSink {
    fn write(&mut self, block: &[f32]) -> bool {
        self.0.lock().unwrap().extend_from_slice(block);
        true
    }
    fn block_frames(&self) -> usize {
        BLOCK
    }
}

/// SPEC v0.4.8 row 14's owed compensation: the master limiter's lookahead
/// (245 frames, 5.1 ms) was carried by program out, the record and the stream
/// "until the device-audio build compensates it". The driver now renders that
/// far ahead, so an impulse taken at master frame 2 leaves the driver at the
/// first frame of frame 2's block, not 245 frames into it.
#[test]
fn the_driver_compensates_the_limiters_lookahead() {
    let state = Arc::new(EngineState::new(30));
    let kept = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut driver = AudioDriver::new(state, Box::new(KeepSink(kept.clone())), 30);
    let mut impulse = vec![0.0f32; 4800 * CHANNELS];
    impulse[0] = 0.5;
    impulse[1] = 0.5;
    driver.graph.set_source(
        nbe_engine::audio::BusId::Clip,
        vec![nbe_engine::audio::Source::Pcm {
            samples: Arc::new(impulse),
            t0: 2,
            looping: false,
            gain: 1.0,
        }],
    );
    for f in 0..4 {
        driver.cycle(f);
    }
    let out = kept.lock().unwrap().clone();
    let peak = out
        .chunks(CHANNELS)
        .enumerate()
        .max_by(|a, b| a.1[0].abs().partial_cmp(&b.1[0].abs()).unwrap())
        .map(|(i, _)| i)
        .unwrap();
    assert_eq!(
        peak,
        2 * BLOCK,
        "the impulse due at master frame 2 leaves at frame 2's first sample (the lookahead is {} frames)",
        driver.graph.latency_frames()
    );
}
