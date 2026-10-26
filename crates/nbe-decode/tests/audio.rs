//! Audio-track decode (Prompt 06b Step 4, SPEC §8.1/§8.9).
//!
//! The fixture's source is mono; the reader is asked for the engine's format,
//! so what comes back must be 48 kHz interleaved stereo regardless.

use nbe_decode::{decode_audio, ENGINE_CHANNELS, ENGINE_SAMPLE_RATE};
use std::path::Path;

fn media(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/media")
        .join(name)
}

#[test]
fn an_assets_audio_decodes_to_the_engine_format() {
    let track = decode_audio(&media("av_tone.mp4"))
        .expect("decode must not error")
        .expect("this fixture has an audio track");
    assert_eq!(track.sample_rate, ENGINE_SAMPLE_RATE);
    assert_eq!(
        track.channels, ENGINE_CHANNELS,
        "a mono source must still arrive in the engine's stereo format"
    );
    // One second, within a little of resampler padding.
    let frames = track.frames();
    assert!(
        frames > 47_000 && frames < 49_500,
        "expected about a second of audio, got {frames} frames"
    );
    // A 1 kHz tone is not silence.
    let peak = track.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.1, "the tone should be audible, peak {peak}");
}

#[test]
fn an_asset_with_no_audio_track_is_not_a_fault() {
    // A silent clip is normal media, not a decode failure.
    let track = decode_audio(&media("cfr_30.mp4")).expect("decode must not error");
    assert!(track.is_none(), "no audio track means None, not Err");
}

#[test]
fn a_corrupt_asset_does_not_yield_a_plausible_track() {
    match decode_audio(&media("corrupt.mp4")) {
        Ok(None) | Err(_) => {}
        Ok(Some(t)) => panic!(
            "a truncated file must not decode to {} frames of audio",
            t.frames()
        ),
    }
}

/// The residual of `x` against the least-squares `hz` sine (amplitude and
/// phase fitted), as dB relative to the fitted tone: what the converter added.
fn sine_residual_db(x: &[f32], hz: f64, rate: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / rate;
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
    let a = (xs * cc - xc * sc) / det;
    let b = (xc * ss - xs * sc) / det;
    let (mut tone, mut resid) = (0.0, 0.0);
    for (n, &v) in x.iter().enumerate() {
        let fit = a * (w * n as f64).sin() + b * (w * n as f64).cos();
        tone += fit * fit;
        resid += (v as f64 - fit) * (v as f64 - fit);
    }
    10.0 * (resid / tone).log10()
}

/// The device-audio design record's D3 (ruled 2026-10-21): the clip's rate
/// converts to the house rate at load, by AVFoundation, exercised on a source
/// that is not 48 kHz, its quality held to a measured bound.
///
/// The fixture: `tone_44k1.wav`, `0.5 · sin(2π · 1000 · n / 44100)` for
/// 44 100 samples, mono IEEE float32, written exactly (sha256 4c4ba8c1…13b2f0).
/// Lossless and exact, so the converter is the only impairment: the same
/// tone at 48 kHz, where nothing converts, reads a -154.9 dB residual through
/// this decoder; ffmpeg's `sine` source read -79.9 dB with no conversion at
/// all, which is why it is not the fixture.
///
/// **The pin, and why there is none** (put to the user with the build): the
/// ruled key, `AVSampleRateConverterAudioQualityKey`, is not supported by
/// `AVAssetReaderTrackOutput` (its header says so, and setting it raises an
/// Objective-C exception that aborts the process). The reader's own knob,
/// `AVSampleRateConverterAlgorithmKey`, pinned to Mastering, measured the same
/// -100.9 dB as the default. So the guard is the measurement itself.
///
/// A source passed through at its own rate but labelled 48 kHz reads 1088 Hz
/// and 44 100 frames; a 16-bit output path would add about -92 dB.
#[test]
fn a_44k1_source_converts_to_48k_at_its_own_pitch_and_length() {
    let track = decode_audio(&media("tone_44k1.wav"))
        .expect("decode must not error")
        .expect("the fixture is audio");
    assert_eq!(track.sample_rate, ENGINE_SAMPLE_RATE);
    assert_eq!(track.channels, ENGINE_CHANNELS);
    let frames = track.frames();
    assert!(
        (47_900..=48_100).contains(&frames),
        "one second at 44.1 kHz is 48 000 frames at 48 kHz, got {frames}"
    );
    // The middle 0.8 s of the left channel, clear of the converter's edges.
    let left: Vec<f32> = track
        .samples
        .chunks(ENGINE_CHANNELS as usize)
        .map(|f| f[0])
        .skip(4_800)
        .take(38_400)
        .collect();
    let crossings = left
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    let hz = crossings as f64 / (left.len() as f64 / ENGINE_SAMPLE_RATE as f64);
    assert!(
        (hz - 1000.0).abs() < 2.0,
        "a 1 kHz tone stays 1 kHz through the conversion, read {hz:.1} Hz"
    );
    let residual = sine_residual_db(&left, 1000.0, ENGINE_SAMPLE_RATE as f64);
    println!("D3-RESIDUAL {residual:.1} dB (frames {frames}, {hz:.2} Hz)");
    assert!(
        residual < -95.0,
        "the converter adds no more than -95 dB to a pure tone (measured -100.9): {residual:.1} dB"
    );
}
