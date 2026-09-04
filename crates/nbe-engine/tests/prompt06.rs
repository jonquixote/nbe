//! Prompt 06 — the audio graph, measured offline.
//!
//! Every test renders samples and measures a number. No audio device is
//! opened, and none is needed: SPEC §8's acceptance criteria are all
//! measurements over samples, and a graph that needs hardware to be tested is
//! a graph that cannot be tested in CI.
//!
//! Each test names the production path it covers and fails when that path is
//! removed (Standards §2a).

use nbe_engine::audio::{
    db_to_linear, linear_to_db, worst_discontinuity_dbfs, AudioGraph, BusId, Source, CHANNELS,
    MIN_RAMP_MS, SAMPLE_RATE,
};
use nbe_engine::audio_control::{apply, AudioCommand};
use std::collections::BTreeMap;
use std::sync::Arc;

#[allow(unsafe_code)]
mod alloc_probe {
    //! A counting global allocator, for the §8.9 allocation gate.
    //!
    //! `unsafe impl GlobalAlloc` is unavoidable here. This is a test binary —
    //! the allocator never ships — and the CI gate that confines unsafe scopes
    //! itself to `src/` for exactly this reason.
    //!
    //! The counters are THREAD-LOCAL. A global counter counts every thread's
    //! allocations, and `cargo test` runs tests in parallel, so the first
    //! version of this gate reported whatever the rest of the suite happened to
    //! be doing. Both cells are const-initialized so touching them cannot
    //! itself allocate.
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
        // `try_with`: during thread teardown the TLS may be gone, and an
        // allocator must never panic.
        let _ = ARMED.try_with(|armed| {
            if armed.get() {
                let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
            }
        });
    }

    /// Run `f` with counting armed on THIS thread, returning the count.
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

const HOUSE_RATE: u32 = 30;

/// One master frame's worth of sample frames at the house rate. Rendering in
/// exact master-frame blocks keeps a tone's phase continuous across blocks, so
/// a discontinuity in the output is the graph's doing and not the harness's.
const BLOCK: usize = SAMPLE_RATE as usize / HOUSE_RATE as usize;

fn buffer(frames: usize) -> Vec<f32> {
    vec![0.0; frames * CHANNELS]
}

/// A stereo tone, as soundboard material.
fn tone_samples(hz: f32, amplitude: f32, frames: usize) -> Arc<Vec<f32>> {
    let mut v = Vec::with_capacity(frames * CHANNELS);
    for n in 0..frames {
        let s = amplitude * (2.0 * std::f32::consts::PI * hz * n as f32 / SAMPLE_RATE as f32).sin();
        for _ in 0..CHANNELS {
            v.push(s);
        }
    }
    Arc::new(v)
}

/// Peak level of a buffer in dBFS.
fn peak_dbfs(buf: &[f32]) -> f32 {
    linear_to_db(buf.iter().fold(0.0f32, |m, s| m.max(s.abs())))
}

/// Peak of the final quarter of a buffer.
///
/// A ramping change is settled by then, so this measures the value the change
/// moved TO. Peak over a whole buffer measures the value it moved FROM, which
/// is what made the first version of the ducking test read 0.9 dB.
fn settled_peak_dbfs(buf: &[f32]) -> f32 {
    let tail = buf.len() / 4 * 3;
    peak_dbfs(&buf[tail..])
}

/// Render two consecutive master frames into ONE contiguous buffer, applying
/// `change` at the join.
///
/// This is the only honest way to measure AC-19. Measuring within a single
/// buffer misses the case that matters: an instant gain change lands exactly
/// at a buffer boundary, and a within-buffer measurement cannot see it. The
/// first version of these tests had that hole, and a falsification pass that
/// deleted ramping entirely still showed them green.
fn render_across_change(
    g: &mut AudioGraph,
    first_frame: u64,
    change: impl FnOnce(&mut AudioGraph),
) -> Vec<f32> {
    let mut buf = buffer(BLOCK * 2);
    let (a, b) = buf.split_at_mut(BLOCK * CHANNELS);
    g.render(a, first_frame);
    change(g);
    g.render(b, first_frame + 1);
    buf
}

// ---------------------------------------------------------------------------
// AC-19 — click-free (SPEC §8.7.1). Path: Ramp + Bus::set_muted/set_gain_db.
// ---------------------------------------------------------------------------

#[test]
fn muting_a_bus_ramps_instead_of_stepping() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: 0.5,
        }],
    );

    // Baseline: two blocks with no change, so the tone's own slope is known.
    let base_buf = render_across_change(&mut g, 0, |_| {});
    let present = peak_dbfs(&base_buf);
    assert!(
        present > -20.0,
        "precondition: the bus must be audible before muting it means \
         anything, got {present:.1} dBFS"
    );
    let baseline = worst_discontinuity_dbfs(&base_buf);

    // The same two blocks with a mute at the join.
    let muted = worst_discontinuity_dbfs(&render_across_change(&mut g, 10, |g| {
        g.bus_mut(BusId::Mic).set_muted(true, 10.0);
    }));

    assert!(
        muted <= baseline + 1.0,
        "muting must not add a discontinuity: baseline {baseline:.1} dBFS, \
         muting {muted:.1} dBFS"
    );
    // And the mute must have happened. Comparing discontinuities alone, a
    // `set_muted` that does nothing is indistinguishable from one that ramps.
    let settled = settled_peak_dbfs(&render_across_change(&mut g, 12, |_| {}));
    assert!(
        settled < -60.0,
        "the bus must actually be muted; still at {settled:.1} dBFS"
    );
    println!("mute: baseline {baseline:.1} dBFS, during mute {muted:.1} dBFS");
}

#[test]
fn a_gain_change_of_60_db_never_steps() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    // DC-ish source: any step shows up directly as a discontinuity.
    g.set_source(
        BusId::Clip,
        vec![Source::Tone {
            hz: 1.0,
            amplitude: 0.9,
        }],
    );
    // A 1 Hz source is near-DC, so its own slope is negligible and any jump in
    // the output is the gain change's doing. A step would move the full 0.9 in
    // one sample (about -1 dBFS); a 5 ms ramp of the same size is far below.
    let before = settled_peak_dbfs(&render_across_change(&mut g, 0, |_| {}));
    let worst = worst_discontinuity_dbfs(&render_across_change(&mut g, 2, |g| {
        g.bus_mut(BusId::Clip).set_gain_db(-60.0, 5.0);
    }));
    assert!(
        worst < -40.0,
        "a -60 dB gain change must ramp, not step; worst jump was {worst:.1} dBFS"
    );
    // And the gain must have moved. Without this a `set_gain_db` that does
    // nothing passes the discontinuity bar trivially.
    let after = settled_peak_dbfs(&render_across_change(&mut g, 4, |_| {}));
    assert!(
        before - after > 50.0,
        "the gain change must land: {before:.1} -> {after:.1} dBFS"
    );
    println!("60 dB change over 5 ms: worst jump {worst:.1} dBFS");
}

#[test]
fn stopping_a_soundboard_voice_ramps_it_out() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    // 442.5 Hz, not 440. The stop lands at the join of the SECOND pair of
    // blocks, which is sample 4800 of the voice — and 440 Hz completes exactly
    // 44 cycles there. That is a zero crossing: the waveform is at 0, so a
    // full cut and a ramp produce the identical number and the test cannot
    // tell them apart. It could not: replacing the stop ramp with an instant
    // cut left this test green. 442.5 Hz puts sample 4800 a quarter cycle
    // later, at the peak — maximum step for a cut, minimum slope for the
    // baseline, which is the only place the two are distinguishable.
    const HZ: f32 = 442.5;
    const AMPLITUDE: f32 = 0.8;
    let samples = tone_samples(HZ, AMPLITUDE, SAMPLE_RATE as usize);
    let id = g.trigger("stab", samples, 0.0);

    // Baseline: the voice running, no change at the join. The tone's own slope
    // is the floor, so the bar is "the stop adds nothing on top of that".
    let base_buf = render_across_change(&mut g, 0, |_| {});
    assert!(peak_dbfs(&base_buf) > -20.0, "the voice should be audible");
    let baseline = worst_discontinuity_dbfs(&base_buf);

    let buf = render_across_change(&mut g, 2, |g| {
        g.stop_voice(Some(id), None);
    });
    // Guard the test's own power, by MEASUREMENT rather than arithmetic. The
    // discrimination exists only because the join lands near a waveform peak:
    // at a zero crossing a full cut and a ramp produce the same number. The
    // previous guard recomputed the phase from a hard-coded "3 preceding
    // blocks", so adding a render moved the real join to a zero crossing while
    // the guard still read 0.25 and passed. This reads the actual last sample
    // before the join, so nothing can move it without failing here.
    let at_join = buf[BLOCK * CHANNELS - 1].abs();
    assert!(
        at_join > AMPLITUDE * 0.75,
        "this test only tells a cut from a ramp when the join sits near a \
         waveform peak; the sample there is {at_join:.3} of {AMPLITUDE}"
    );

    let worst = worst_discontinuity_dbfs(&buf);
    assert!(
        worst <= baseline + 1.0,
        "stopping a voice must ramp, not cut: baseline {baseline:.1} dBFS, \
         during stop {worst:.1} dBFS"
    );

    // And the stop must actually finish. Without this, a `stop_voice` that
    // does nothing at all also clears the discontinuity bar — the ramp
    // assertion alone cannot tell "ramped out" from "never stopped".
    let settled = settled_peak_dbfs(&buf[BLOCK * CHANNELS..]);
    assert!(
        settled < -60.0,
        "a 10 ms stop must be silent long before a 33 ms block ends; the \
         voice is still at {settled:.1} dBFS"
    );
    println!(
        "voice stop: baseline {baseline:.1} dBFS, during stop {worst:.1} dBFS, \
         settled {settled:.1} dBFS"
    );
}

// ---------------------------------------------------------------------------
// AC-18 — mix-minus isolation (SPEC §8.6). Path: render_guest_return.
// ---------------------------------------------------------------------------

#[test]
fn a_guests_own_audio_is_absent_from_its_return_and_present_in_others() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    // -20 dBFS 1 kHz on guest G only; nothing else in the show.
    g.upsert_guest(
        "G",
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: db_to_linear(-20.0),
        }],
    );
    g.upsert_guest("H", vec![]);

    let mut own = buffer(4096);
    g.render_guest_return("G", &mut own, 0);
    let leaked = peak_dbfs(&own);
    assert!(
        leaked <= -80.0,
        "AC-18: guest G's own tone must be at or below -80 dBFS in its own \
         return, measured {leaked:.1} dBFS"
    );

    // And the isolation is specific, not a silent graph: H hears G.
    let mut other = buffer(4096);
    g.render_guest_return("H", &mut other, 0);
    let heard = peak_dbfs(&other);
    assert!(
        heard > -30.0,
        "guest H must hear guest G; measured {heard:.1} dBFS"
    );
}

// ---------------------------------------------------------------------------
// AC-13 — soundboard latency (SPEC §8.4). Path: AudioGraph::trigger.
// ---------------------------------------------------------------------------

#[test]
fn a_soundboard_trigger_is_audible_within_20ms() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    let samples = tone_samples(1000.0, 0.7, SAMPLE_RATE as usize);
    g.trigger("stab", samples, 0.0);

    // 20 ms at 48 kHz.
    let budget_frames = (SAMPLE_RATE as usize * 20) / 1000;
    let mut buf = buffer(budget_frames);
    g.render(&mut buf, 0);

    // `position` cannot exceed the buffer, so `first_audible < budget_frames`
    // was tautological — the `expect` was doing all the work. Assert the AC's
    // real bar instead: audible within a couple of samples of the trigger.
    let first_audible = buf
        .as_chunks::<CHANNELS>()
        .0
        .iter()
        .position(|f| f.iter().any(|s| s.abs() > 0.001))
        .expect("the trigger must produce output inside the 20 ms budget");
    assert!(
        first_audible <= 2,
        "AC-13: a trigger is audible immediately, not {first_audible} samples in"
    );
    // Immediate, but not a click: §8.7.1 ramps the voice in over the 5 ms
    // floor. Starting a voice at full scale — the click the code comment names
    // — left 116/116 green, since it is audible even sooner.
    let head = buf[0].abs();
    let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(
        peak > 0.5,
        "precondition: the stab must reach full level, got {peak:.3}"
    );
    assert!(
        head < peak * 0.1,
        "a trigger must ramp in, not start at full scale: first sample \
         {head:.4} against a peak of {peak:.3}"
    );
    // Report the measured number, which is what the AC is about.
    println!(
        "AC-13 trigger latency: {first_audible} samples ({:.2} ms)",
        first_audible as f32 * 1000.0 / SAMPLE_RATE as f32
    );
}

// ---------------------------------------------------------------------------
// Ducking (SPEC §8.3). Path: AudioGraph::set_duck + the music bus.
// ---------------------------------------------------------------------------

#[test]
fn ducking_attenuates_music_by_its_depth_and_recovers() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Music,
        vec![Source::Tone {
            hz: 220.0,
            amplitude: 0.5,
        }],
    );

    let mut buf = buffer(4096);
    g.render(&mut buf, 0);
    let unducked = settled_peak_dbfs(&buf);

    // Duck 6 dB with a 10 ms attack, then measure once the attack has settled.
    g.set_duck(true, Some(-6.0), Some(10.0), Some(250.0));
    let mut buf = buffer(4096);
    g.render(&mut buf, 4);
    let ducked = settled_peak_dbfs(&buf);

    let attenuation = unducked - ducked;
    assert!(
        (attenuation - 6.0).abs() < 1.5,
        "ducking must attenuate by ~6 dB, measured {attenuation:.1} dB \
         (unducked {unducked:.1}, ducked {ducked:.1})"
    );

    // Release recovers.
    g.set_duck(false, None, None, Some(50.0));
    let mut buf = buffer(8192);
    g.render(&mut buf, 8);
    let recovered = settled_peak_dbfs(&buf);
    assert!(
        (recovered - unducked).abs() < 1.5,
        "music must recover after release: {recovered:.1} vs {unducked:.1}"
    );
}

#[test]
fn ducking_leaves_mic_and_guest_alone() {
    // `settled_peak_dbfs`, not `peak_dbfs`. Measured over the whole buffer the
    // head is the level from before the 10 ms attack lands, so a bus wrongly
    // ducked still reports its old peak: ducking every program bus in
    // violation of §8.3 moved the whole-buffer number by 0.38 dB and left this
    // test green, while the settled tail moved the full 6.00 dB. The helper's
    // own doc comment warns about exactly this trap; it was fixed in the music
    // test and left here.
    let mic_before = mic_settled_level(false);
    let mic_after = mic_settled_level(true);
    // Absolute guard first. `linear_to_db` floors at -120 rather than -inf, so
    // silence-vs-silence reads as a delta of 0.0 and passes every relative
    // assertion below: deleting the guest summing loop from `render()`
    // entirely — no remote guest ever heard on the program mix — left the
    // whole suite green. A delta is only evidence if the signal is there.
    assert!(
        mic_before > -20.0,
        "precondition: the mic must be audible in the program mix, got {mic_before:.1} dBFS"
    );
    assert!(
        (mic_before - mic_after).abs() < 0.5,
        "SPEC §8.3: ducking must not touch the mic bus ({mic_before:.1} -> {mic_after:.1})"
    );

    // And the guest half of the name, which was never tested at all — no guest
    // bus was ever created here.
    let guest_before = guest_settled_level(false);
    let guest_after = guest_settled_level(true);
    assert!(
        guest_before > -20.0,
        "precondition: a guest must be audible in the program mix, got \
         {guest_before:.1} dBFS — SPEC §8.1 sums guest buses into master"
    );
    assert!(
        (guest_before - guest_after).abs() < 0.5,
        "SPEC §8.3: ducking must not touch a guest bus ({guest_before:.1} -> {guest_after:.1})"
    );
    println!(
        "duck isolation: mic {mic_before:.1} -> {mic_after:.1} dBFS, \
         guest {guest_before:.1} -> {guest_after:.1} dBFS"
    );
}

/// The mic bus alone, measured after any ramp has settled. `duck` engages the
/// ducker; nothing else differs between the two runs.
fn mic_settled_level(duck: bool) -> f32 {
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 300.0,
            amplitude: 0.5,
        }],
    );
    if duck {
        g.set_duck(true, Some(-6.0), Some(10.0), Some(250.0));
    }
    let mut buf = buffer(4096);
    g.render(&mut buf, if duck { 4 } else { 0 });
    settled_peak_dbfs(&buf)
}

/// One guest bus alone, same shape.
fn guest_settled_level(duck: bool) -> f32 {
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.upsert_guest(
        "G",
        vec![Source::Tone {
            hz: 700.0,
            amplitude: 0.5,
        }],
    );
    if duck {
        g.set_duck(true, Some(-6.0), Some(10.0), Some(250.0));
    }
    let mut buf = buffer(4096);
    g.render(&mut buf, if duck { 4 } else { 0 });
    settled_peak_dbfs(&buf)
}

// ---------------------------------------------------------------------------
// SPEC §8.9 — audio follows the master clock, and there is only one clock.
// ---------------------------------------------------------------------------

#[test]
fn a_sources_read_offset_comes_from_the_master_clock() {
    let g = AudioGraph::new(HOUSE_RATE);
    // One second of show time is one second of samples.
    assert_eq!(g.sample_for_master_frame(30, 0), SAMPLE_RATE as i64);
    // An item that went on air at frame 30 is at its own sample 0 there.
    assert_eq!(g.sample_for_master_frame(30, 30), 0);
    // And 15 frames later, half a second in.
    assert_eq!(g.sample_for_master_frame(45, 30), SAMPLE_RATE as i64 / 2);
}

#[test]
fn the_house_rate_is_configuration_not_a_constant() {
    // SPEC §8.9: sampleForMasterFrame(F) = (F - t0) * sampleRate / houseRate.
    // A literal 30 in that arithmetic gives 40000 here, and mis-times every
    // frame of a 25 fps show.
    let g25 = AudioGraph::new(25);
    assert_eq!(
        g25.sample_for_master_frame(25, 0),
        SAMPLE_RATE as i64,
        "one second of show time at 25 fps is one second of samples"
    );
    assert_eq!(g25.sample_for_master_frame(50, 25), SAMPLE_RATE as i64);

    // And a source's own t0 offset uses the same rate: an item taken at frame
    // 25 of a 25 fps show is at its own sample 0 one second in.
    let mut g = AudioGraph::new(25);
    let samples: Arc<Vec<f32>> = tone_samples(1000.0, 0.5, SAMPLE_RATE as usize);
    g.set_source(
        BusId::Clip,
        vec![Source::Pcm {
            samples,
            t0: 25,
            looping: false,
        }],
    );
    let mut buf = buffer(512);
    g.render(&mut buf, 25);
    // At its own frame 0 the tone starts at zero and rises; if the offset were
    // computed with a literal 30 the read would land 8000 samples away and the
    // first sample would not be near zero.
    // Not just "quiet": an all-zero buffer satisfies a low-side bound, so
    // assert the signal exists elsewhere in the buffer first.
    let alive = peak_dbfs(&buf);
    assert!(
        alive > -20.0,
        "precondition: the source must be audible, got {alive:.1} dBFS"
    );
    assert!(
        buf[0].abs() < 0.01,
        "a 25 fps item at its own t0 must start at its first sample, got {}",
        buf[0]
    );
}

#[test]
fn drift_is_measured_against_the_master_clock() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    // Render exactly one second of audio.
    let mut buf = buffer(SAMPLE_RATE as usize);
    g.render(&mut buf, 0);
    // At master frame 30 the clock also implies one second: no drift.
    let drift = g.drift_ms(30);
    assert!(drift.abs() < 0.001, "expected no drift, got {drift} ms");

    // If the clock has moved on but audio has not, the drift is negative and
    // measured — SPEC §11.5.1's ±1 frame budget is 33 ms at 30 fps.
    let behind = g.drift_ms(60);
    assert!(
        behind < -900.0 && behind > -1100.0,
        "a second behind should measure about -1000 ms, got {behind}"
    );
}

// ---------------------------------------------------------------------------
// SPEC §8.10 — underruns are counted, and they are NOT a video fault.
// ---------------------------------------------------------------------------

#[test]
fn an_underrun_is_counted_and_never_blacks_the_view() {
    use nbe_engine::state::EngineState;
    let state = Arc::new(EngineState::new(HOUSE_RATE));

    // Drive the underruns through the production path. The first version of
    // this test built a bare `AudioGraph`, which holds no state handle, and
    // stored the count into a fresh `EngineState` by hand — so `fallback_active`
    // was a field nothing in the test could ever set, and the §10.3 assertion
    // below could not fail. Making `AudioDriver::cycle` raise the fallback on
    // an underrun left it green. Now the sink refuses every block and the
    // driver owns the state, so the assertion is reachable.
    struct DeadSink;
    impl nbe_engine::audio_driver::AudioSink for DeadSink {
        fn write(&mut self, _block: &[f32]) -> bool {
            false
        }
        fn block_frames(&self) -> usize {
            BLOCK
        }
    }
    let mut driver = AudioDriver::new(state.clone(), Box::new(DeadSink), HOUSE_RATE);
    for frame in 0..5 {
        driver.cycle(frame);
    }

    assert_eq!(
        state
            .audio_underruns_total
            .load(std::sync::atomic::Ordering::SeqCst),
        5
    );
    // SPEC §10.3 is a *video* watchdog: an audio fault must not put the
    // fallback slate on air. Cutting the picture because audio glitched turns
    // a small fault into a visible one.
    assert!(
        !state
            .fallback_active
            .load(std::sync::atomic::Ordering::SeqCst),
        "an audio underrun must not activate the video fallback"
    );

    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } =
        nbe_engine::telemetry::build_tick(&state)
    else {
        panic!("expected EngineTelemetry");
    };
    assert_eq!(fields.audio_underruns_total, 5, "underruns reach telemetry");
}

// ---------------------------------------------------------------------------
// Metering and the bus enum (SPEC §8.1, §8.2, §10.1).
// ---------------------------------------------------------------------------

#[test]
fn bus_peaks_are_metered_per_bus() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: db_to_linear(-6.0),
        }],
    );
    let mut buf = buffer(4096);
    g.render(&mut buf, 0);

    let peaks = g.bus_peaks();
    let mic = peaks.get("mic").copied().expect("mic bus is metered");
    assert!(
        (mic - -6.0).abs() < 1.0,
        "mic peak should be about -6 dBFS, measured {mic:.1}"
    );
    assert!(peaks.contains_key("master"), "master bus is metered");
}

#[test]
fn bus_names_match_the_spec_table_and_the_control_planes_enum() {
    // SPEC §8.1's bus table.
    let spec_buses = [
        "mic",
        "clip",
        "music",
        "sfx",
        "guest",
        "master",
        "guestReturn",
        "ifb",
    ];
    let ours: Vec<&str> = BusId::ALL.iter().map(|b| b.as_str()).collect();
    assert_eq!(ours, spec_buses, "bus names must match SPEC §8.1");

    // And the control plane's `audio.bus.set` enum, which validates these
    // names before they ever reach the engine. Two spellings of guestReturn
    // is a bug that only shows up on air.
    let ts = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packages/control-plane/src/protocol.ts"
    ))
    .expect("control-plane protocol is readable");
    let start = ts
        .find("bus: z.enum([")
        .expect("audio.bus.set enum present");
    let end = ts[start..].find("])").expect("enum terminates") + start;
    for name in spec_buses {
        assert!(
            ts[start..end].contains(&format!("\"{name}\"")),
            "the control plane's audio.bus.set enum is missing {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// The directive path (SPEC §16.8): intents cross as data, not as calls.
// ---------------------------------------------------------------------------

#[test]
fn audio_directives_parse_into_intents_and_apply_to_the_graph() {
    use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
    let frame = |command: &str, payload: serde_json::Value| DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: 0,
        state_version: 1,
        command: command.into(),
        target: serde_json::json!({}),
        payload,
    };

    let cmd = AudioCommand::from_directive(&frame(
        "audio.bus.set",
        serde_json::json!({ "bus": "music", "gainDb": -12.0 }),
    ))
    .unwrap();
    assert_eq!(
        cmd,
        AudioCommand::BusSet {
            bus: "music".into(),
            guest_id: None,
            gain_db: Some(-12.0),
            muted: None
        }
    );

    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Music,
        vec![Source::Tone {
            hz: 220.0,
            amplitude: 0.5,
        }],
    );
    let mut before = buffer(4096);
    g.render(&mut before, 0);
    let loud = settled_peak_dbfs(&before);

    apply(&mut g, vec![cmd], &BTreeMap::new());
    let mut after = buffer(8192);
    g.render(&mut after, 4);
    let quiet = settled_peak_dbfs(&after);

    assert!(
        loud - quiet > 8.0,
        "a -12 dB bus set must be audible in the output: {loud:.1} -> {quiet:.1}"
    );
}

#[test]
fn soundboard_play_uses_only_resident_samples() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    let mut library = BTreeMap::new();
    library.insert("stab".to_string(), tone_samples(880.0, 0.6, 4800));

    // An asset that is not resident produces no voice and no panic: the
    // engine never reads from disk here (SPEC §8.9).
    apply(
        &mut g,
        vec![AudioCommand::Play {
            asset_id: "missing".into(),
            gain_db: 0.0,
        }],
        &library,
    );
    assert_eq!(g.active_voices(), 0);

    apply(
        &mut g,
        vec![AudioCommand::Play {
            asset_id: "stab".into(),
            gain_db: 0.0,
        }],
        &library,
    );
    assert_eq!(g.active_voices(), 1);
}

// ---------------------------------------------------------------------------
// SPEC §8.9 — the callback allocates nothing. The gate, not the comment.
// ---------------------------------------------------------------------------

#[test]
fn rendering_a_block_allocates_nothing() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: 0.3,
        }],
    );
    g.set_source(
        BusId::Music,
        vec![Source::Tone {
            hz: 220.0,
            amplitude: 0.3,
        }],
    );
    g.upsert_guest(
        "G",
        vec![Source::Tone {
            hz: 500.0,
            amplitude: 0.2,
        }],
    );
    g.upsert_guest("H", vec![]);
    g.trigger("stab", tone_samples(880.0, 0.5, 48_000), 0.0);

    let mut buf = buffer(BLOCK);
    // Warm up: `reserve` may grow the scratch buffers the first time it sees
    // this block size, which is setup, not steady state.
    g.render(&mut buf, 0);

    let allocs = allocations_during(|| {
        g.render(&mut buf, 1);
    });
    assert_eq!(
        allocs, 0,
        "AudioGraph::render must allocate nothing in the steady state \
         (SPEC §8.9); it made {allocs} allocations"
    );

    let mut ret = buffer(BLOCK);
    let allocs = allocations_during(|| {
        g.render_guest_return("G", &mut ret, 1);
    });
    assert_eq!(
        allocs, 0,
        "render_guest_return must allocate nothing either; it made {allocs}"
    );
}

// ---------------------------------------------------------------------------
// SPEC §8.6/§8.7.1 — a change landing mid-buffer must ramp in the IFB path.
// ---------------------------------------------------------------------------

#[test]
fn a_mute_mid_buffer_ramps_in_the_guest_return() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: 0.5,
        }],
    );
    g.upsert_guest("G", vec![]);

    // Two consecutive blocks with the mic muted at the join. Applying the
    // gain once per buffer would step here.
    let mut buf = buffer(BLOCK * 2);
    let (a, b) = buf.split_at_mut(BLOCK * CHANNELS);
    g.render_guest_return("G", a, 0);
    // Precondition: the mic must actually be IN the return. Deleting the whole
    // program-bus loop of `render_guest_return` — so no guest ever hears the
    // host mic, clips, music or SFX in their IFB — left 116/116 green, because
    // every assertion here is a delta and `linear_to_db` floors at -120.0, so
    // silence-vs-silence compares equal.
    let present = peak_dbfs(a);
    assert!(
        present > -20.0,
        "SPEC §8.6: the program mix must be present in a guest's return, \
         measured {present:.1} dBFS"
    );
    let baseline = worst_discontinuity_dbfs(a);
    g.bus_mut(BusId::Mic).set_muted(true, 10.0);
    g.render_guest_return("G", b, 1);

    let worst = worst_discontinuity_dbfs(&buf);
    assert!(
        worst <= baseline + 1.0,
        "a mute landing mid-stream must ramp in the guest return too: \
         baseline {baseline:.1} dBFS, with mute {worst:.1} dBFS"
    );

    // And the ramp must actually RUN. A gain applied once per buffer advances
    // the ramp one sample per block, so a 10 ms mute would take 240 blocks to
    // land — no step to measure, and the mic still audible in the IFB long
    // after the operator muted it.
    let settled = settled_peak_dbfs(&buf[BLOCK * CHANNELS..]);
    assert!(
        settled < -60.0,
        "a 10 ms mute must be complete within a 33 ms block; the guest return \
         still carries the mic at {settled:.1} dBFS"
    );
    println!(
        "guest-return mid-buffer mute: baseline {baseline:.1} dBFS, with mute \
         {worst:.1} dBFS, settled {settled:.1} dBFS"
    );
}

#[test]
fn a_guest_mute_mid_buffer_ramps_in_every_other_return() {
    // The test above covers the program-bus loop of `render_guest_return`.
    // This covers the guest-bus loop, which is the same code shape and had
    // nothing watching it: hoisting `next_gain()` out of its per-frame loop
    // left the whole suite green. `guest.mute` and
    // `audio.bus.set{bus:"guestReturn"}` both land on `guest_bus_mut`, so a
    // guest muted mid-block would step in every other guest's IFB.
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.upsert_guest("G", vec![]); // the listener
    g.upsert_guest(
        "H",
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: 0.5,
        }],
    );

    let mut buf = buffer(BLOCK * 2);
    let (a, b) = buf.split_at_mut(BLOCK * CHANNELS);
    g.render_guest_return("G", a, 0);
    let present = peak_dbfs(a);
    assert!(
        present > -20.0,
        "precondition: G must hear H before muting H means anything, got \
         {present:.1} dBFS"
    );
    let baseline = worst_discontinuity_dbfs(a);
    g.guest_bus_mut("H")
        .expect("guest H exists")
        .set_muted(true, 10.0);
    g.render_guest_return("G", b, 1);

    let worst = worst_discontinuity_dbfs(&buf);
    assert!(
        worst <= baseline + 1.0,
        "muting a guest must ramp in the other guests' returns: baseline \
         {baseline:.1} dBFS, with mute {worst:.1} dBFS"
    );

    let settled = settled_peak_dbfs(&buf[BLOCK * CHANNELS..]);
    assert!(
        settled < -60.0,
        "a 10 ms guest mute must be complete within a 33 ms block; G still \
         hears H at {settled:.1} dBFS"
    );
    println!(
        "guest-bus mid-buffer mute: baseline {baseline:.1} dBFS, with mute \
         {worst:.1} dBFS, settled {settled:.1} dBFS"
    );
}

// ---------------------------------------------------------------------------
// Step 2 — the driver. Path: AudioDriver::cycle -> drain -> render -> publish.
// A graph nothing drives is a mechanism test; these cover the driving.
// ---------------------------------------------------------------------------

use nbe_engine::audio_driver::{AudioDriver, NullSink};
use nbe_engine::state::EngineState;

fn driver_with_null_sink(state: Arc<EngineState>) -> AudioDriver {
    AudioDriver::new(state, Box::new(NullSink::new(BLOCK)), HOUSE_RATE)
}

#[test]
fn the_driver_drains_intents_and_they_reach_the_graph() {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let mut driver = driver_with_null_sink(state.clone());
    driver.graph.set_source(
        BusId::Music,
        vec![Source::Tone {
            hz: 220.0,
            amplitude: 0.5,
        }],
    );

    // A directive-side publish, exactly as `on_audio` does it.
    state
        .audio_commands
        .lock()
        .unwrap()
        .push(AudioCommand::BusSet {
            bus: "music".into(),
            guest_id: None,
            gain_db: Some(-40.0),
            muted: None,
        });

    // Before the cycle, the queue is full and nothing has been applied.
    assert_eq!(state.audio_commands.lock().unwrap().len(), 1);
    driver.cycle(0);
    assert!(
        state.audio_commands.lock().unwrap().is_empty(),
        "the driver must drain what the directive path publishes"
    );

    // And the change is audible: render past the ramp, then read what the
    // driver PUBLISHED. Reading the graph's own meters here would read them
    // after `publish` reset them — always silence, and a test that passes
    // whether or not the intent was ever applied.
    for f in 1..8 {
        driver.cycle(f);
    }
    let music = state
        .bus_peaks
        .lock()
        .unwrap()
        .get("music")
        .copied()
        .expect("music is metered");
    assert!(
        music < -30.0,
        "a -40 dB bus set must reach the graph; music peaked at {music:.1} dBFS"
    );
    // Not vacuous: without the gain change this bus is loud. Read the
    // PUBLISHED peaks here too — the graph's own meters are reset by publish.
    let fresh_state = Arc::new(EngineState::new(HOUSE_RATE));
    let mut fresh = driver_with_null_sink(fresh_state.clone());
    fresh.graph.set_source(
        BusId::Music,
        vec![Source::Tone {
            hz: 220.0,
            amplitude: 0.5,
        }],
    );
    fresh.cycle(0);
    let loud = fresh_state
        .bus_peaks
        .lock()
        .unwrap()
        .get("music")
        .copied()
        .unwrap_or(-120.0);
    assert!(
        loud > -30.0,
        "the unchanged bus must be loud, else the assertion above proves nothing"
    );
}

#[test]
fn the_driver_publishes_the_v0_3_3_telemetry_fields() {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let mut driver = driver_with_null_sink(state.clone());
    driver.graph.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: db_to_linear(-6.0),
        }],
    );

    driver.cycle(0);

    // Production wrote these, not a test.
    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } =
        nbe_engine::telemetry::build_tick(&state)
    else {
        panic!("expected EngineTelemetry");
    };
    let mic = fields
        .bus_peak_dbfs
        .get("mic")
        .copied()
        .expect("busPeakDbfs must carry the mic bus");
    assert!(
        (mic - -6.0).abs() < 1.5,
        "the driver must publish real peaks; mic reported {mic:.1} dBFS"
    );
    assert_eq!(fields.audio_underruns_total, 0);
    assert!(
        fields.audio_drift_ms.abs() < 40.0,
        "one block in, drift should be under a frame: {} ms",
        fields.audio_drift_ms
    );

    // And drift must be MEASURED, not defaulted. `< 40.0` is satisfied by the
    // 0.0 of a field nobody wrote: deleting the drift store from `publish`
    // left 116/116 green. Cycling three times against a standing master frame
    // means the graph has rendered three blocks the clock did not ask for, so
    // real drift is 3 blocks = 100 ms.
    for _ in 0..3 {
        driver.cycle(0);
    }
    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } =
        nbe_engine::telemetry::build_tick(&state)
    else {
        panic!("expected EngineTelemetry");
    };
    let expected = driver.graph.drift_ms(0);
    assert!(
        expected > 90.0,
        "the fixture must actually produce drift, got {expected:.1} ms"
    );
    assert!(
        (fields.audio_drift_ms - expected).abs() < 0.001,
        "the driver must publish the graph's measured drift: telemetry {} ms \
         vs graph {expected} ms",
        fields.audio_drift_ms
    );
}

#[test]
fn the_master_stage_limits_instead_of_clipping() {
    // SPEC §8.2: master gain and metering, and a limiter rather than a
    // clipper. None of it was tested — deleting the entire master stage from
    // `render()` left 116/116 green, because the only assertion touching it
    // was `peaks.contains_key("master")`, which the bus map satisfies whether
    // the stage runs or not.
    let mut g = AudioGraph::new(HOUSE_RATE);
    // Four buses at 0.8 sum to 3.2 — well past full scale.
    for id in [BusId::Mic, BusId::Clip, BusId::Music, BusId::Guest] {
        g.set_source(
            id,
            vec![Source::Tone {
                hz: 220.0,
                amplitude: 0.8,
            }],
        );
    }
    let mut buf = buffer(BLOCK);
    g.render(&mut buf, 0);

    let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(
        peak > 0.5,
        "precondition: the sum must actually be loud, got {peak:.3}"
    );
    assert!(
        peak <= 1.0,
        "the master stage must limit: a sample reached {peak:.3}, and hard \
         clipping is a click generator"
    );
    let metered = g
        .bus_peaks()
        .get("master")
        .copied()
        .expect("master metered");
    assert!(
        metered > -20.0,
        "the master bus must meter what it passed, got {metered:.1} dBFS"
    );

    // And master gain must actually apply.
    g.bus_mut(BusId::Master).set_gain_db(-40.0, 5.0);
    let quiet = settled_peak_dbfs(&render_across_change(&mut g, 1, |_| {}));
    assert!(
        quiet < -20.0,
        "master gain must reach the output; still at {quiet:.1} dBFS"
    );
}

#[test]
fn a_ramp_shorter_than_the_floor_is_still_a_ramp() {
    // SPEC §8.7.1/§8.7.6: `Ramp` enforces a 5 ms floor, which is what makes
    // "a cut still ramps" true. Deleting the clamp from `Ramp::to` left
    // 116/116 green even though a sibling test asserts the rule in prose.
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Clip,
        vec![Source::Tone {
            hz: 1.0,
            amplitude: 0.9,
        }],
    );
    // A 1 Hz source is near-DC: any step is the gain change's doing.
    let worst = worst_discontinuity_dbfs(&render_across_change(&mut g, 0, |g| {
        g.bus_mut(BusId::Clip).set_gain_db(-60.0, 0.0);
    }));
    assert!(
        worst < -40.0,
        "a 0 ms ramp must still take the {MIN_RAMP_MS} ms floor, not step; \
         worst jump {worst:.1} dBFS"
    );
    let settled = settled_peak_dbfs(&render_across_change(&mut g, 2, |_| {}));
    assert!(
        settled < -40.0,
        "and the change must land: {settled:.1} dBFS"
    );
}

#[test]
fn peaks_are_windowed_so_a_meter_falls_again() {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let mut driver = driver_with_null_sink(state.clone());
    driver.graph.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: 0.9,
        }],
    );
    driver.cycle(0);
    let loud = state.bus_peaks.lock().unwrap().get("mic").copied().unwrap();

    // Silence the source; the next window must report silence, not the
    // loudest moment since the show began.
    driver.graph.set_source(BusId::Mic, Vec::new());
    driver.cycle(1);
    let quiet = state.bus_peaks.lock().unwrap().get("mic").copied().unwrap();

    assert!(
        loud > -3.0,
        "the tone should have peaked hot, got {loud:.1}"
    );
    assert!(
        quiet < -60.0,
        "peaks are per-interval; a meter that never falls tells an operator \
         nothing after the first transient (got {quiet:.1} dBFS)"
    );
}

#[test]
fn a_sink_that_cannot_take_a_block_is_an_underrun_and_not_a_video_fault() {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let mut sink = NullSink::new(BLOCK);
    sink.fail_next = true;
    let mut driver = AudioDriver::new(state.clone(), Box::new(sink), HOUSE_RATE);

    driver.cycle(0);

    assert_eq!(
        state
            .audio_underruns_total
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a block the sink refused is an underrun (SPEC §8.10)"
    );
    assert!(
        !state
            .fallback_active
            .load(std::sync::atomic::Ordering::SeqCst),
        "SPEC §10.3: an audio fault must never put the fallback slate on air"
    );
}

#[tokio::test]
async fn the_spawned_driver_runs_without_anyone_pumping_it() {
    // `spawn` is the production entry point. This proves the task it creates
    // actually cycles: intents queued before it starts get drained by it and
    // telemetry appears, with nobody calling `cycle` by hand.
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    state
        .audio_commands
        .lock()
        .unwrap()
        .push(AudioCommand::StopAll);

    let handle = nbe_engine::audio_driver::spawn(state.clone(), HOUSE_RATE);
    let mut drained = false;
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        if state.audio_commands.lock().unwrap().is_empty()
            && !state.bus_peaks.lock().unwrap().is_empty()
        {
            drained = true;
            break;
        }
    }
    handle.abort();
    assert!(
        drained,
        "the spawned driver must drain intents and publish peaks on its own"
    );
}

#[test]
fn the_engine_binary_actually_starts_the_audio_driver() {
    // This spawns the real binary and waits for the driver to announce itself.
    //
    // The previous version of this gate searched main.rs for the text
    // `audio_driver::spawn(`. An independent pass defeated it four ways — the
    // call inside a string literal, inside `if false`, behind `#[cfg(any())]`,
    // behind `#[cfg(test)]` — each leaving 116/116 green, clippy clean, and
    // the engine never starting the driver. A substring test over source
    // cannot tell a reachable call from a token sequence, so it is gone. This
    // observes the running process instead, and the only way to pass it is to
    // start the driver.
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = Command::new(env!("CARGO_BIN_EXE_nbe-engine"))
        .env("NBE_RENDER_TOKEN", "test-token")
        .env("NBE_CP_URL", "ws://127.0.0.1:1/nbe/v0.3") // nothing listening, by design
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the engine binary must be runnable");

    let mut out = child.stdout.take().expect("stdout piped");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let mut chunk = [0u8; 1024];
        loop {
            match out.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    if buf.contains("audio driver started") {
                        let _ = tx.send(true);
                        return;
                    }
                }
            }
        }
        let _ = tx.send(false);
    });

    let started = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap_or(false);
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        started,
        "the engine binary must start the audio driver; without it the audio \
         graph is a mechanism nothing drives (SPEC §8.9)"
    );
}

#[test]
fn the_drain_is_bounded_so_a_flood_cannot_starve_a_block() {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let mut driver = driver_with_null_sink(state.clone());
    {
        let mut q = state.audio_commands.lock().unwrap();
        for _ in 0..500 {
            q.push(AudioCommand::StopAll);
        }
    }
    driver.cycle(0);
    let left = state.audio_commands.lock().unwrap().len();
    assert_eq!(
        left,
        500 - nbe_engine::audio_driver::MAX_COMMANDS_PER_CYCLE,
        "the drain is bounded per cycle; an unbounded one turns a command \
         flood into a missed block"
    );
}

// ---------------------------------------------------------------------------
// Step 4 — the take's audio object (SPEC §8.7.3, §8.7.5, §8.7.6).
// ---------------------------------------------------------------------------

#[test]
fn the_take_audio_modes_map_to_gain_and_ramp() {
    use nbe_engine::audio_control::take_gain_and_ramp;

    // §8.7.6: a cut still ramps — `Ramp` enforces the 5 ms floor.
    assert_eq!(take_gain_and_ramp("cut", 10.0, 0, 30), (0.0, 10.0));
    // §8.7.5: a mix crossfades over the video's own duration. 15 frames at
    // 30 fps is 500 ms.
    let (gain, ramp) = take_gain_and_ramp("crossfade", 10.0, 15, 30);
    assert_eq!(gain, 0.0);
    assert!((ramp - 500.0).abs() < 0.01, "crossfade ramp was {ramp} ms");
    // `mute` takes the item off the mix.
    assert_eq!(take_gain_and_ramp("mute", 10.0, 0, 30).0, -60.0);
    // `follow` is AFV: the item's policy, which defaults to clip audio at unity.
    assert_eq!(take_gain_and_ramp("follow", 10.0, 0, 30), (0.0, 10.0));
    // And the house rate is honoured, not assumed: 15 frames at 25 fps is 600 ms.
    let (_, ramp25) = take_gain_and_ramp("crossfade", 10.0, 15, 25);
    assert!(
        (ramp25 - 600.0).abs() < 0.01,
        "at 25 fps the ramp was {ramp25} ms"
    );
}

#[test]
fn a_muted_take_silences_the_clip_bus_without_a_step() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    let samples = tone_samples(440.0, 0.8, SAMPLE_RATE as usize);
    let mut library = BTreeMap::new();
    library.insert("A1".to_string(), samples);

    apply(
        &mut g,
        vec![AudioCommand::TakeItem {
            item_ref: "A1".into(),
            t0: 0,
            mode: "follow".into(),
            ramp_ms: 10.0,
            crossfade_frames: 0,
        }],
        &library,
    );
    let base_buf = render_across_change(&mut g, 0, |_| {});
    let audible = settled_peak_dbfs(&base_buf);
    assert!(
        audible > -20.0,
        "the taken item should be audible: {audible:.1}"
    );
    // The tone's own slope, measured — not an absolute bar. `worst < -20.0`
    // was cleared by the 440 Hz slope (-26.7 dBFS) whatever the code did.
    let baseline = worst_discontinuity_dbfs(&base_buf);

    // Now a muted take of the same item.
    let buf = render_across_change(&mut g, 2, |g| {
        apply(
            g,
            vec![AudioCommand::TakeItem {
                item_ref: "A1".into(),
                t0: 2,
                mode: "mute".into(),
                ramp_ms: 10.0,
                crossfade_frames: 0,
            }],
            &library,
        );
    });
    let silenced = settled_peak_dbfs(&buf);
    assert!(
        silenced < -50.0,
        "a muted take must silence the clip bus, got {silenced:.1} dBFS"
    );
    // And it got there by ramping (§8.7.1).
    let worst = worst_discontinuity_dbfs(&buf);
    assert!(
        worst <= baseline + 1.0,
        "a muted take must ramp, not step: baseline {baseline:.1} dBFS, \
         muted take {worst:.1} dBFS"
    );
    // What this test gates is `swap_source_through_silence` (removing it fails
    // here). `set_gain_db`'s own ramp is gated by
    // `a_gain_change_of_60_db_never_steps`, whose near-DC source makes a step
    // unmissable; this join cannot see it, because the swap has already taken
    // the bus through silence by the time the gain moves.
    println!("muted take: baseline {baseline:.1} dBFS, muted {worst:.1} dBFS");
}
