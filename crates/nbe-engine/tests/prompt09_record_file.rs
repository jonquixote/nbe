//! Prompt 09 WU34 (SPEC §9.3): crash-safe recording file output.
//!
//! TDD: this test was written BEFORE the implementation (RED first).
//! `nbe_engine::record` does not exist yet — this file must fail to compile
//! until the module tree lands, proving the tests actually exercise new code.
//!
//! Coverage maps to the work-unit DoD:
//! 1. `write_recording` → fMP4 parses under ffprobe with 1×h264 + 1×aac,
//!    duration within (1 video frame + 3 AAC frames + 10 ms container
//!    rounding) of expected, tone audio present (astats).
//! 2. Fragments ≤1 s (box walk), ftyp+moov upfront (init safe), dropped
//!    writer without `finish` still parses (kill -9 shape, AC-6 at unit level).
//! 3. Audio tap: bounded ring, overfill drops oldest + counts, never blocks.
//! 4. Audio is real AAC (`codec_name=aac`), never PCM-in-MP4; missing
//!    AudioToolbox fails loudly.
//! 5. `E_DISK` on an unwritable target via thiserror.
//!
//! ffprobe lives at `/usr/local/bin/ffprobe` (9.0.1). If the binary is absent
//! the tests print a skip note and return — the ONLY allowed skip.

use std::path::{Path, PathBuf};
use std::process::Command;

use nbe_engine::encode::{is_available as hw_available, EncodeSession};
use nbe_engine::record::{write_recording, AudioTap, RecordParams, RecordingWriter};

const FFPROBE: &str = "/usr/local/bin/ffprobe";
const FFMPEG: &str = "/usr/local/bin/ffmpeg";
const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const FPS: u32 = 30;
const FRAMES_2S: u32 = 60;
const SAMPLE_RATE: u32 = 48_000;

fn ffprobe_or_skip() -> Option<PathBuf> {
    let p = PathBuf::from(FFPROBE);
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("SKIP: {FFPROBE} absent — recording file tests need ffprobe 9.0.1");
        None
    }
}

fn ffprobe_json(ffprobe: &Path, extra: &[&str], file: &Path) -> serde_json::Value {
    let mut cmd = Command::new(ffprobe);
    cmd.arg("-v")
        .arg("error")
        .args(extra)
        .arg("-of")
        .arg("json")
        .arg(file);
    let out = cmd.output().expect("spawning ffprobe must succeed");
    assert!(
        out.status.success(),
        "ffprobe must parse the recording, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("ffprobe JSON must parse")
}

fn synthetic_rgba(width: u32, height: u32, frame: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            out.push(((x + frame * 7) % 256) as u8);
            out.push(((y + frame * 13) % 256) as u8);
            out.push(((x + y + frame * 3) % 256) as u8);
            out.push(255);
        }
    }
    out
}

/// Real H.264 access units from the hardware encoder (WU2), 2 s @ 30 fps,
/// plus the SPS/PPS captured from the first keyframe's format description
/// (`EncodeSession::parameter_sets`) for the file's `avcC`.
///
/// Platform fact this design rests on (verified, not assumed): VideoToolbox
/// emits NO parameter sets in-band — a dumped keyframe is SEI + IDR only —
/// so the sets ride in [`RecordParams`] instead of being parsed from units.
fn encode_two_seconds() -> (Vec<nbe_engine::encode::EncodedUnit>, Vec<u8>, Vec<u8>) {
    assert!(
        hw_available(),
        "LOUD FAILURE: no hardware H.264 encoder (SPEC §9.2); recording has no CPU fallback"
    );
    let mut session = EncodeSession::open(WIDTH, HEIGHT, FPS, 1_000_000)
        .expect("EncodeSession::open must succeed where hardware exists");
    for frame in 0..FRAMES_2S {
        session
            .encode_rgba(&synthetic_rgba(WIDTH, HEIGHT, frame))
            .expect("feeding RGBA must succeed");
    }
    let sets = session
        .parameter_sets()
        .expect("LOUD FAILURE: first IDR exposed no SPS/PPS; no avcC without them");
    let units = session.finish().expect("finish must complete the stream");
    assert!(
        units.iter().any(|u| u.is_keyframe),
        "stream must open with a keyframe"
    );
    let (sps, pps) = sets;
    assert_eq!(sps[0] & 0x1F, 7, "first set must be an SPS");
    assert_eq!(pps[0] & 0x1F, 8, "second set must be a PPS");
    (units, sps, pps)
}

/// 2 s stereo 440 Hz tone, amplitude 0.5, interleaved f32 — verifiably NOT silence.
fn tone_two_seconds() -> Vec<f32> {
    let frames = (SAMPLE_RATE * 2) as usize;
    let mut pcm = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / SAMPLE_RATE as f32).sin();
        pcm.push(v);
        pcm.push(v);
    }
    pcm
}

fn params(dir: &Path, sps: Vec<u8>, pps: Vec<u8>) -> RecordParams {
    RecordParams {
        directory: dir.to_path_buf(),
        show: "demoshow".into(),
        episode: "ep01".into(),
        start_timestamp: "20260914T120000Z".into(),
        width: WIDTH,
        height: HEIGHT,
        fps: FPS,
        sps,
        pps,
    }
}

/// Top-level box walk: Vec<(fourcc, box_len, body_offset)>.
fn top_level_boxes(bytes: &[u8]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let kind = String::from_utf8_lossy(&bytes[off + 4..off + 8]).into_owned();
        let len = if len == 1 {
            u64::from_be_bytes(bytes[off + 8..off + 16].try_into().unwrap()) as usize
        } else {
            len
        };
        assert!(len >= 8, "box {kind} has impossible length {len}");
        out.push((kind, len));
        off += len;
    }
    out
}

/// All `moof` payloads with their file offsets, for fragment walk.
fn moof_ranges(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let kind = &bytes[off + 4..off + 8];
        let len = if len == 1 {
            u64::from_be_bytes(bytes[off + 8..off + 16].try_into().unwrap()) as usize
        } else {
            len
        };
        if len < 8 {
            break;
        }
        if kind == b"moof" {
            out.push((off, len));
        }
        off += len;
    }
    out
}

/// Sum of video-track (track_ID 1) sample durations inside one moof, in seconds.
/// The writer emits per-sample duration/size/flags/cts in every video trun
/// (flags 0x0F01), tfdt carries the base decode time; this walk reads that
/// layout back to prove each fragment spans ≤ 1 s.
fn moof_video_duration_secs(moof: &[u8]) -> f64 {
    const VIDEO_TIMESCALE: f64 = 90_000.0;
    let mut total = 0u64;
    let mut off = 8usize; // skip moof header
    while off + 8 <= moof.len() {
        let len = u32::from_be_bytes(moof[off..off + 4].try_into().unwrap()) as usize;
        let kind = &moof[off + 4..off + 8];
        if len < 8 || off + len > moof.len() {
            break;
        }
        if kind == b"traf" {
            total += traf_video_duration(&moof[off..off + len]);
        }
        off += len;
    }
    total as f64 / VIDEO_TIMESCALE
}

fn traf_video_duration(traf: &[u8]) -> u64 {
    let mut off = 8usize;
    let mut is_video = false;
    let mut duration = 0u64;
    while off + 8 <= traf.len() {
        let len = u32::from_be_bytes(traf[off..off + 4].try_into().unwrap()) as usize;
        let kind = &traf[off + 4..off + 8];
        if len < 8 || off + len > traf.len() {
            break;
        }
        let body = &traf[off..off + len];
        if kind == b"tfhd" {
            // flags(1)+version... tfhd body: version+flags then track_ID.
            let track_id = u32::from_be_bytes(body[12..16].try_into().unwrap());
            is_video = track_id == 1;
        } else if kind == b"trun" && is_video {
            // version+flags at [8..12]; sample_count at [12..16].
            let flags = u32::from_be_bytes(body[8..12].try_into().unwrap()) & 0x00FF_FFFF;
            assert_eq!(
                flags, 0x0F01,
                "video trun must carry duration+size+flags+cts per sample"
            );
            let n = u32::from_be_bytes(body[12..16].try_into().unwrap()) as usize;
            // data_offset(4) precedes the samples.
            let mut p = 20usize;
            for _ in 0..n {
                assert!(p + 16 <= body.len(), "trun sample runs past box end");
                duration += u32::from_be_bytes(body[p..p + 4].try_into().unwrap()) as u64;
                p += 16; // duration(4) size(4) flags(4) cts(4)
            }
        }
        off += len;
    }
    duration
}

#[test]
fn write_recording_produces_parseable_h264_aac_file() {
    let Some(ffprobe) = ffprobe_or_skip() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let (units, sps, pps) = encode_two_seconds();
    let pcm = tone_two_seconds();

    let path = write_recording(&units, &pcm, &params(dir.path(), sps, pps))
        .expect("write_recording must succeed on a writable target");

    // Filename derives from show/episode + start timestamp.
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        "demoshow_ep01_20260914T120000Z.mp4"
    );
    assert_eq!(path.parent().unwrap(), dir.path());

    let v = ffprobe_json(&ffprobe, &["-show_streams", "-show_format"], &path);
    let streams = v["streams"].as_array().expect("streams array");
    assert_eq!(streams.len(), 2, "exactly 1 video + 1 audio stream");
    let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
    let audio = streams.iter().find(|s| s["codec_type"] == "audio").unwrap();
    assert_eq!(video["codec_name"], "h264");
    assert_eq!(video["width"], 640);
    assert_eq!(video["height"], 360);
    // DoD 4: real AAC, never PCM-in-MP4.
    assert_eq!(
        audio["codec_name"], "aac",
        "audio must be AAC frames, not PCM-in-MP4"
    );
    assert_eq!(audio["sample_rate"], "48000");

    // Duration within (1 video frame + 3 AAC frames + 10 ms container
    // rounding) of the 2 s fed in — the bound is named for what it is, not
    // "within 1 frame". Documented AAC realities: the encoder carries its
    // ~2112-sample priming delay, of which the writer trims 2 packets (2048
    // samples) at mux time, leaving a 64-sample / 1.33 ms residual; the audio
    // track holds ceil(2*48000/1024)+2−2 = 94 packets = 2.005 s against
    // exactly 2.0 s of video. The bound is one video frame + three AAC
    // frames + 10 ms of container rounding.
    let dur: f64 = v["format"]["duration"]
        .as_str()
        .expect("format duration")
        .parse()
        .expect("duration parses");
    let bound = 1.0 / FPS as f64 + 3.0 * 1024.0 / SAMPLE_RATE as f64 + 0.01;
    assert!(
        (dur - 2.0).abs() <= bound,
        "duration {dur} must be within 1 video frame + 3 AAC frames + 10 ms ({bound}s) of 2.0 s"
    );
}

#[test]
fn fragments_are_sub_second_and_init_segment_first() {
    let Some(_) = ffprobe_or_skip() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let (units, sps, pps) = encode_two_seconds();
    let pcm = tone_two_seconds();
    let path = write_recording(&units, &pcm, &params(dir.path(), sps, pps)).unwrap();

    let bytes = std::fs::read(&path).expect("recording must be readable");
    let top = top_level_boxes(&bytes);
    assert!(top.len() >= 3, "ftyp + moov + ≥1 moof, got {top:?}");
    assert_eq!(top[0].0, "ftyp", "init segment opens with ftyp");
    assert_eq!(top[1].0, "moov", "moov upfront: init-segment safe");

    // 2 s at ≤1 s fragments ⇒ ≥2 moofs, each spanning ≤1 s of video.
    let moofs = moof_ranges(&bytes);
    assert!(
        moofs.len() >= 2,
        "2 s of content needs ≥2 fragments, found {}",
        moofs.len()
    );
    for (off, len) in &moofs {
        let d = moof_video_duration_secs(&bytes[*off..off + len]);
        assert!(
            d <= 1.0 + 1.0 / FPS as f64,
            "fragment video span {d}s exceeds 1 s + 1 frame"
        );
    }
}

#[test]
fn drop_without_finish_leaves_prior_fragments_parseable() {
    // The AC-6 shape at unit level: kill -9 mid-write == dropping the writer
    // without `finish`. Prior flushed fragments must still parse.
    let Some(ffprobe) = ffprobe_or_skip() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let (units, sps, pps) = encode_two_seconds();
    let pcm = tone_two_seconds();
    let p = params(dir.path(), sps, pps);

    let split_at = units.len() * 3 / 4;
    let path = {
        let mut w = RecordingWriter::create(&p).expect("create must succeed");
        for u in &units[..split_at] {
            w.push_video(u).expect("push_video must succeed");
        }
        // Feed the audio covering the same span (interleaved per §9.3).
        let audio_frames = (split_at as f64 / FPS as f64 * SAMPLE_RATE as f64) as usize;
        w.push_audio(&pcm[..audio_frames * 2])
            .expect("push_audio must succeed");
        let path = w.path().to_path_buf();
        // No finish: the crash shape. Tail (if any) is discarded, flushed
        // fragments stay.
        std::mem::drop(w);
        path
    };

    let bytes = std::fs::read(&path).expect("partial file must exist");
    let top = top_level_boxes(&bytes);
    assert_eq!(top[0].0, "ftyp");
    assert_eq!(top[1].0, "moov");
    assert!(
        !moof_ranges(&bytes).is_empty(),
        "a partial write must retain ≥1 flushed fragment"
    );

    let v = ffprobe_json(&ffprobe, &["-show_streams", "-show_format"], &path);
    let streams = v["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2, "partial file keeps both streams");
    assert!(streams.iter().any(|s| s["codec_name"] == "h264"));
    assert!(streams.iter().any(|s| s["codec_name"] == "aac"));
}

#[test]
fn recorded_audio_is_tone_not_silence() {
    // Mean abs amplitude > 0, measured through ffprobe's astats: a tone's
    // Overall Max_level sits far above 0 while silence reports -inf. Feeding
    // the encoder silence would pass every structural check and fail this one.
    let Some(ffprobe) = ffprobe_or_skip() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let (units, sps, pps) = encode_two_seconds();
    let pcm = tone_two_seconds();
    let path = write_recording(&units, &pcm, &params(dir.path(), sps, pps)).unwrap();

    // Audio is stream 1 (video is 0): select it explicitly. Default
    // per-packet astats windows (NOT reset=0, whose cumulative tags read 0
    // on this ffprobe build): priming packets report 0, content packets
    // report the tone — the max over frames is the assertion.
    let lavfi = format!("amovie={}:si=1,astats=metadata=1", path.display());
    let out = Command::new(&ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(&lavfi)
        .arg("-show_entries")
        .arg("frame_tags")
        .arg("-of")
        .arg("json")
        .output()
        .expect("ffprobe astats must spawn");
    assert!(out.status.success(), "astats probe must succeed");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let frames = v["frames"].as_array().expect("astats frames");
    assert!(!frames.is_empty(), "astats must observe audio frames");
    let peak = frames
        .iter()
        .filter_map(|f| {
            f["tags"]["lavfi.astats.Overall.Max_level"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
        })
        .fold(0.0f64, f64::max);
    assert!(
        peak > 0.01,
        "tone peak {peak} must clear 0.01 (silence reports 0/-inf)"
    );
}

#[test]
fn unwritable_target_reports_e_disk() {
    // A regular file where the output directory should be: creating the
    // recording fails deterministically for any uid (no chmod games).
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let blocker = dir.path().join("not-a-dir");
    std::fs::write(&blocker, b"x").unwrap();
    let (units, sps, pps) = encode_two_seconds();
    let mut p = params(&blocker, sps, pps);
    p.directory = blocker.clone();
    let pcm = tone_two_seconds();

    let err = write_recording(&units, &pcm, &p).expect_err("must fail on unwritable target");
    let msg = err.to_string();
    assert!(
        msg.contains("E_DISK"),
        "error must carry E_DISK, got: {msg}"
    );
    assert!(
        matches!(err, nbe_engine::record::RecordError::Disk(_)),
        "error variant must be RecordError::Disk"
    );
}

#[test]
fn audio_tap_ring_drops_oldest_counts_and_never_blocks() {
    // Bounded ring, drop-oldest policy: overfill by 4 on capacity 8 drops the
    // first 4 pushed and counts them; the caller is never asked to wait —
    // push takes &[f32], performs no I/O, waits on no condition variable.
    let tap = AudioTap::with_capacity(8);
    tap.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    assert_eq!(tap.dropped(), 0);
    tap.push(&[7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
    assert_eq!(tap.dropped(), 4, "overfill by 4 drops the 4 oldest");
    let got = tap.drain();
    assert_eq!(got, vec![5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
    assert!(tap.drain().is_empty(), "drain empties the ring");

    // Hammer from threads: 8 pushers × 5k pushes against a draining reader.
    // Completion itself is the assertion — a blocking or deadlocking tap
    // would hang this test; drops are expected and counted, never fatal.
    let tap = std::sync::Arc::new(AudioTap::with_capacity(4096));
    let mut handles = Vec::new();
    for t in 0..8 {
        let tap = tap.clone();
        handles.push(std::thread::spawn(move || {
            let buf = vec![t as f32; 256];
            for _ in 0..5000 {
                tap.push(&buf);
            }
        }));
    }
    let reader = std::thread::spawn({
        let tap = tap.clone();
        move || {
            let mut total = 0usize;
            for _ in 0..2000 {
                total += tap.drain().len();
            }
            total
        }
    });
    for h in handles {
        h.join().expect("pushers must finish");
    }
    reader.join().expect("reader must finish");
    eprintln!("tap drops under contention: {}", tap.dropped());
}

#[test]
fn rendered_master_mix_arrives_in_tap() {
    // Tap call-site proof: the master-bus post-render mix pushes a copy into
    // the tap. Render the graph with a tap attached, drain, and the frames
    // must arrive non-empty with sizes matching the rendered block.
    use nbe_engine::audio::{AudioGraph, BusId, Source};
    use std::sync::Arc;

    let tap = Arc::new(AudioTap::with_capacity(48_000 * 2 * 5));
    let mut g = AudioGraph::new(30);
    g.set_record_tap(tap.clone());
    g.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 440.0,
            amplitude: 0.5,
        }],
    );

    let mut out = vec![0.0f32; 480 * 2];
    g.render(&mut out, 0);
    assert!(
        out.iter().any(|v| v.abs() > 0.01),
        "rendered master mix must carry the tone"
    );
    let got = tap.drain();
    assert!(!got.is_empty(), "rendered frames must arrive in the tap");
    assert_eq!(
        got.len(),
        out.len(),
        "tap must hold exactly one block's copy (sizes match)"
    );
    assert_eq!(got, out, "tap copy must equal the post-master mix");

    // Second block appends a second copy: the tap is fed every render.
    let mut out2 = vec![0.0f32; 480 * 2];
    g.render(&mut out2, 1);
    let got2 = tap.drain();
    assert_eq!(got2.len(), out2.len(), "second render feeds the tap again");
    assert_eq!(got2, out2);
}

/// 1 s silence + 1 s 440 Hz tone, stereo interleaved f32 — a known audio
/// event (tone onset) at exactly 1.0 s against video starting at 0.
fn silence_then_tone() -> Vec<f32> {
    let mut pcm = Vec::with_capacity((SAMPLE_RATE * 2) as usize * 2);
    for _ in 0..SAMPLE_RATE {
        pcm.push(0.0);
        pcm.push(0.0);
    }
    for n in 0..SAMPLE_RATE {
        let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / SAMPLE_RATE as f32).sin();
        pcm.push(v);
        pcm.push(v);
    }
    pcm
}

/// Decode the file's audio track to stereo f32le via ffmpeg and return the
/// first frame index where the tone has clearly started (3 consecutive
/// frames above threshold, to reject codec idling noise). Time = frame / rate.
fn decoded_tone_onset_secs(path: &Path) -> f64 {
    let out = Command::new(FFMPEG)
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-map")
        .arg("0:a")
        .arg("-f")
        .arg("f32le")
        .arg("-acodec")
        .arg("pcm_f32le")
        .arg("-ac")
        .arg("2")
        .arg("-ar")
        .arg("48000")
        .arg("-")
        .output()
        .expect("spawning ffmpeg to decode audio must succeed");
    assert!(out.status.success(), "ffmpeg audio decode must succeed");
    assert!(
        out.stdout.len().is_multiple_of(4),
        "ffmpeg f32le bytes must frame-align"
    );
    let n = out.stdout.len() / 4;
    let mut samples = Vec::with_capacity(n);
    let (chunks, _) = out.stdout.as_chunks::<4>();
    for chunk in chunks {
        samples.push(f32::from_le_bytes(*chunk));
    }
    let frames = samples.len() / 2;
    let mut run = 0usize;
    for f in 0..frames {
        let peak = samples[f * 2].abs().max(samples[f * 2 + 1].abs());
        if peak > 0.1 {
            run += 1;
            if run >= 3 {
                return (f + 1 - run) as f64 / SAMPLE_RATE as f64;
            }
        } else {
            run = 0;
        }
    }
    panic!("no tone onset found in decoded audio ({} frames)", frames);
}

#[test]
fn av_sync_offset_within_20ms_after_priming_trim() {
    // A/V sync: the writer drops the first 2 AAC packets (2048 of the 2112
    // priming samples) at mux time, leaving a 64-sample / 1.33 ms residual.
    // The known audio event (tone onset at 1.0 s) must decode within 20 ms of
    // video time 1.0 s. Untrimmed (~44 ms delay) fails this bound.
    let Some(ffprobe) = ffprobe_or_skip() else {
        return;
    };
    if !PathBuf::from(FFMPEG).is_file() {
        eprintln!("SKIP: {FFMPEG} absent — A/V sync test needs ffmpeg decode");
        return;
    }
    let _ = &ffprobe;
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let (units, sps, pps) = encode_two_seconds();
    let pcm = silence_then_tone();
    let path = write_recording(&units, &pcm, &params(dir.path(), sps, pps))
        .expect("write_recording must succeed");

    let onset = decoded_tone_onset_secs(&path);
    let offset = (onset - 1.0).abs();
    eprintln!("tone onset at {onset:.4}s vs video 1.0s (offset {offset:.4}s)");
    assert!(
        offset <= 0.020,
        "A/V offset {offset:.4}s exceeds 20 ms (onset {onset:.4}s vs 1.0 s)"
    );
}
