//! A take's audio and video cover the same span of the show (R53's root cause).
//!
//! Before: the video's PTS were the encoder's submit count, so a frame the take
//! skipped pulled every later frame earlier and shortened the video; the audio
//! was every block the driver pushed between the tap's attach and detach, in
//! the driver's processing order, so a driver running behind at either end
//! lengthened or shortened it (`record_tap_boundary.rs`). The two tracks of
//! one take could differ by a hundred milliseconds or more, and the file's
//! length was whichever was longer.
//!
//! Now both are on the master clock: a video frame's PTS is its master frame
//! less the take's first, so a skipped frame is a held frame; the tap places
//! each block it is given on the master clock, and the record thread cuts the
//! audio to `[first video frame, last video frame + 1)`, padding with silence
//! where the tap fell short.
//!
//! Needs the hardware H.264 encoder, AudioToolbox AAC and ffprobe; each test
//! says SKIP when one is absent.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use nbe_engine::record::{AudioTap, RecordMsg, RecordSession};

const W: u32 = 640;
const H: u32 = 360;
const FPS: u32 = 30;
/// Master audio frames per master video frame at 48 kHz and 30 fps.
const SPF: i64 = 48_000 / FPS as i64;
const FFPROBE: &str = "/usr/local/bin/ffprobe";
/// One AAC packet, in seconds: the audio track's length is whole packets.
const PACKET_S: f64 = 1024.0 / 48_000.0;

fn ready() -> Option<PathBuf> {
    if !nbe_engine::encode::is_available() {
        eprintln!("SKIP: no hardware H.264 encoder on this machine");
        return None;
    }
    if !nbe_engine::record::aac::is_available() {
        eprintln!("SKIP: AudioToolbox AAC absent");
        return None;
    }
    let ffprobe = PathBuf::from(FFPROBE);
    if !ffprobe.exists() {
        eprintln!("SKIP: {FFPROBE} absent");
        return None;
    }
    Some(ffprobe)
}

/// A frame whose content changes every frame, so each encodes to a real unit.
fn frame_rgba(i: u64) -> Vec<u8> {
    let i = i as u32;
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for (p, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (p as u32) % W;
        let y = (p as u32) / W;
        px[0] = ((x + i * 7) % 256) as u8;
        px[1] = ((y + i * 3) % 256) as u8;
        px[2] = ((x ^ y ^ i) % 256) as u8;
        px[3] = 255;
    }
    rgba
}

/// `frames` of a quiet tone, interleaved stereo.
fn tone(frames: i64) -> Vec<f32> {
    (0..frames)
        .flat_map(|k| {
            let v = 0.1 * (2.0 * std::f32::consts::PI * 440.0 * k as f32 / 48_000.0).sin();
            [v, v]
        })
        .collect()
}

/// Record the master frames `handed` (in order), with the tap given `audio`
/// samples placed at master audio frame `audio_from`. Returns the file.
fn take(dir: &Path, handed: &[u64], audio_from: i64, audio: &[f32]) -> PathBuf {
    let tap = Arc::new(AudioTap::new());
    tap.push_at(audio, audio_from);
    let skipped = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut session = RecordSession::open(
        dir,
        "spanshow",
        "ep01",
        "20261121T000000Z",
        W,
        H,
        FPS,
        tap,
        skipped.clone(),
    )
    .expect("the take opens");
    let tx = session.frame_sender();
    for &f in handed {
        tx.send(RecordMsg::Frame {
            rgba: frame_rgba(f),
            frame: Some(f),
        })
        .expect("the record thread takes the frame");
    }
    drop(tx);
    let path = session
        .stop_and_finish(Duration::from_secs(20))
        .expect("the take finishes");
    assert_eq!(
        skipped.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "precondition: the thread shed no frame"
    );
    path
}

/// (video duration, audio duration) in seconds, from each stream's own entry.
fn durations(ffprobe: &Path, file: &Path) -> (f64, f64) {
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(file)
        .output()
        .expect("ffprobe runs");
    assert!(out.status.success(), "ffprobe parses the take");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("ffprobe JSON");
    let of = |kind: &str| -> f64 {
        v["streams"]
            .as_array()
            .expect("streams")
            .iter()
            .find(|s| s["codec_type"] == kind)
            .and_then(|s| s["duration"].as_str())
            .and_then(|d| d.parse().ok())
            .unwrap_or_else(|| panic!("a {kind} stream with a duration"))
    };
    (of("video"), of("audio"))
}

/// Master frames 100..=144 with 110-112 skipped: 42 handed, spanning 45.
fn handed_with_a_skip() -> Vec<u64> {
    (100..=144).filter(|f| !(110..=112).contains(f)).collect()
}

#[test]
fn a_skipped_frame_is_held_and_the_audio_is_cut_to_the_video_span() {
    let Some(ffprobe) = ready() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let handed = handed_with_a_skip();
    // The tap holds audio from 5 frames before the take's first to 30 past
    // its last: what a driver running behind at the attach and still running
    // at the stop would give it.
    let from = 95 * SPF;
    let path = take(dir.path(), &handed, from, &tone(80 * SPF));
    let (video, audio) = durations(&ffprobe, &path);
    let span = 45.0 / f64::from(FPS);
    println!("span {span:.6} s: video {video:.6} s, audio {audio:.6} s");
    assert!(
        (video - span).abs() < 0.001,
        "the video covers the take's 45-frame span ({span:.6} s), the 3 skipped frames held: {video:.6} s"
    );
    assert!(
        audio >= span - 0.001 && audio <= span + PACKET_S + 0.001,
        "the audio is cut to the same span, give or take its last AAC packet: {audio:.6} s for {span:.6} s"
    );
}

#[test]
fn audio_that_ends_before_the_last_frame_is_padded_to_the_video_span() {
    let Some(ffprobe) = ready() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let handed: Vec<u64> = (100..=144).collect();
    // The tap holds audio for the take's first 30 frames only.
    let path = take(dir.path(), &handed, 100 * SPF, &tone(30 * SPF));
    let (video, audio) = durations(&ffprobe, &path);
    let span = 45.0 / f64::from(FPS);
    println!("span {span:.6} s: video {video:.6} s, audio {audio:.6} s");
    assert!((video - span).abs() < 0.001, "video {video:.6} s for {span:.6} s");
    assert!(
        audio >= span - 0.001 && audio <= span + PACKET_S + 0.001,
        "the audio is padded with silence to the video's span: {audio:.6} s for {span:.6} s"
    );
}
