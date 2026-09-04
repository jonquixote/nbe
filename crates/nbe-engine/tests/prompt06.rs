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
    SAMPLE_RATE,
};
use nbe_engine::audio_control::{apply, AudioCommand};
use std::collections::BTreeMap;
use std::sync::Arc;

const HOUSE_RATE: u32 = 30;

fn buffer(frames: usize) -> Vec<f32> {
    vec![0.0; frames * CHANNELS]
}

/// A one-second stereo tone at full-ish level, as soundboard material.
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

/// One master frame's worth of sample frames at the house rate. Rendering in
/// exact master-frame blocks keeps a tone's phase continuous across blocks, so
/// a discontinuity in the output is the graph's doing and not the harness's.
const BLOCK: usize = SAMPLE_RATE as usize / HOUSE_RATE as usize;

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

/// Peak of the final quarter of a buffer.
///
/// A ramping change is settled by then, so this measures the value the change
/// moved TO. Peak over a whole buffer measures the value it moved FROM, which
/// is what made the first version of the ducking test read 0.9 dB.
fn settled_peak_dbfs(buf: &[f32]) -> f32 {
    let tail = buf.len() / 4 * 3;
    peak_dbfs(&buf[tail..])
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
    let baseline = worst_discontinuity_dbfs(&render_across_change(&mut g, 0, |_| {}));

    // The same two blocks with a mute at the join.
    let muted = worst_discontinuity_dbfs(&render_across_change(&mut g, 10, |g| {
        g.bus_mut(BusId::Mic).set_muted(true, 10.0);
    }));

    assert!(
        muted <= baseline + 1.0,
        "muting must not add a discontinuity: baseline {baseline:.1} dBFS, \
         muting {muted:.1} dBFS"
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
    let worst = worst_discontinuity_dbfs(&render_across_change(&mut g, 0, |g| {
        g.bus_mut(BusId::Clip).set_gain_db(-60.0, 5.0);
    }));
    assert!(
        worst < -40.0,
        "a -60 dB gain change must ramp, not step; worst jump was {worst:.1} dBFS"
    );
    println!("60 dB change over 5 ms: worst jump {worst:.1} dBFS");
}

#[test]
fn stopping_a_soundboard_voice_ramps_it_out() {
    let mut g = AudioGraph::new(HOUSE_RATE);
    let samples = tone_samples(440.0, 0.8, SAMPLE_RATE as usize);
    let id = g.trigger("stab", samples, 0.0);

    // Baseline: the voice running, no change at the join. A 440 Hz tone at 0.8
    // already moves about 0.046 per sample, so the bar is "the stop adds
    // nothing on top of that".
    let base_buf = render_across_change(&mut g, 0, |_| {});
    assert!(peak_dbfs(&base_buf) > -20.0, "the voice should be audible");
    let baseline = worst_discontinuity_dbfs(&base_buf);

    let worst = worst_discontinuity_dbfs(&render_across_change(&mut g, 2, |g| {
        g.stop_voice(Some(id), None);
    }));
    assert!(
        worst <= baseline + 1.0,
        "stopping a voice must ramp, not cut: baseline {baseline:.1} dBFS, \
         during stop {worst:.1} dBFS"
    );
    println!("voice stop: baseline {baseline:.1} dBFS, during stop {worst:.1} dBFS");
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

    let first_audible = buf
        .as_chunks::<CHANNELS>()
        .0
        .iter()
        .position(|f| f.iter().any(|s| s.abs() > 0.001))
        .expect("the trigger must produce output inside the budget");
    assert!(
        first_audible < budget_frames,
        "AC-13: first audible sample at frame {first_audible} of {budget_frames}"
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
    let mut g = AudioGraph::new(HOUSE_RATE);
    g.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 300.0,
            amplitude: 0.5,
        }],
    );
    let mut before = buffer(4096);
    g.render(&mut before, 0);
    let mic_before = peak_dbfs(&before);

    g.set_duck(true, Some(-6.0), Some(10.0), Some(250.0));
    let mut after = buffer(4096);
    g.render(&mut after, 4);
    let mic_after = peak_dbfs(&after);

    assert!(
        (mic_before - mic_after).abs() < 0.5,
        "SPEC §8.3: ducking must not touch the mic bus ({mic_before:.1} -> {mic_after:.1})"
    );
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
    let mut g = AudioGraph::new(HOUSE_RATE);

    for _ in 0..5 {
        g.note_underrun();
    }
    state
        .audio_underruns_total
        .store(g.underruns(), std::sync::atomic::Ordering::SeqCst);

    assert_eq!(g.underruns(), 5);
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
fn bus_peaks_are_metered_and_reach_telemetry_shape() {
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
