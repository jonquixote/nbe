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
