//! Prompt 09 WU-pipe (SPEC §9.3, §16.14): dedicated record thread.
//!
//! TDD: written BEFORE `record::thread` exists (RED first). This file must
//! fail to compile until `thread.rs` + the `RecordSession` pipeline rewrite
//! land, proving the tests exercise new code.
//!
//! Target architecture: a dedicated RECORD THREAD owns the `!Send` handles
//! (VT `EncodeSession` + `RecordingWriter` + AAC converter) as thread-locals.
//! The loop never encodes inline: it pre-checks budget, reads back, and hands
//! the frame off over a bounded channel (shed + count on full). The thread
//! encodes, muxes a fragment IMMEDIATELY (crash-safe), and drains the shared
//! [`AudioTap`].
//!
//! Coverage:
//! 1. `Unit` seam: synthetic units + tone through the REAL thread + stop
//!    finishes a moov-carrying file + always-sidecar with NO hardware video.
//! 2. `abandon` (the `force=true` immediate-stop path): flushed fragments are
//!    kept as-is, no finish, no sidecar.
//! 3. Stop with zero frames is loud (`E_RECORD_INPUT`), never an empty file
//!    masquerading as a recording.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nbe_engine::record::session as session_glue;
use nbe_engine::record::{AudioTap, RecordParams};

fn synthetic_params(dir: &Path) -> RecordParams {
    RecordParams {
        directory: dir.to_path_buf(),
        show: "demoshow".into(),
        episode: "ep01".into(),
        start_timestamp: "20260914T120000Z".into(),
        width: 640,
        height: 360,
        fps: 30,
        // Writer-valid dummy sets (NAL types 7/8): the thread uses these for
        // the `Unit` seam exactly as the writer uses caller-supplied sets.
        sps: vec![0x67, 0x64, 0x00, 0x1E, 0xAA],
        pps: vec![0x68, 0x11, 0x22],
    }
}

fn fake_unit(pts_seconds: f64, keyframe: bool) -> nbe_engine::encode::EncodedUnit {
    nbe_engine::encode::EncodedUnit {
        data: if keyframe {
            vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x11, 0x22, 0x33]
        } else {
            vec![0x00, 0x00, 0x00, 0x04, 0x41, 0x11, 0x22, 0x33]
        },
        pts_seconds,
        is_keyframe: keyframe,
    }
}

/// 440 Hz stereo tone, amplitude 0.5, interleaved f32.
fn tone_secs(secs: u32) -> Vec<f32> {
    let frames = (48_000 * secs) as usize;
    let mut pcm = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48_000.0).sin();
        pcm.push(v);
        pcm.push(v);
    }
    pcm
}

fn aac_or_skip() -> bool {
    if nbe_engine::record::aac::is_available() {
        true
    } else {
        eprintln!("SKIP: AudioToolbox AAC absent — thread tests need the encoder");
        false
    }
}

fn top_level_boxes(bytes: &[u8]) -> Vec<String> {
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
        out.push(kind);
        off += len;
    }
    out
}

fn assert_moov_present(path: &Path) {
    let bytes = std::fs::read(path).expect("recording file must exist after finish");
    assert!(!bytes.is_empty(), "recording file must be non-empty");
    let top = top_level_boxes(&bytes);
    assert_eq!(top.first().map(String::as_str), Some("ftyp"));
    assert!(
        top.iter().any(|k| k == "moov"),
        "recording must carry moov, top-level: {top:?}"
    );
}

/// Thread-side skip counter not wired to engine state (unit scope).
fn skipped_counter() -> Arc<std::sync::atomic::AtomicU64> {
    Arc::new(std::sync::atomic::AtomicU64::new(0))
}

/// The record tests share process-wide seams (the marker store), so they
/// serialize on this lock (cf. `prompt09_markers.rs`).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The `Unit` seam finishes hermetically: synthetic units + tone through the
/// REAL record thread (spawn, channel, drain, stop-and-finish) with no
/// hardware video touched. Markers ride the always-sidecar.
#[test]
fn unit_seam_through_real_thread_finishes_file_and_sidecar() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    nbe_engine::record::markers::add(nbe_engine::record::markers::Marker {
        name: "intro".into(),
        frame: 30,
        timecode: None,
    });
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let params = synthetic_params(dir.path());
    let tap = Arc::new(AudioTap::new());
    tap.push(&tone_secs(1));
    let mut session = session_glue::RecordSession::open_with_sets(
        &params.directory,
        &params.show,
        &params.episode,
        &params.start_timestamp,
        params.width,
        params.height,
        params.fps,
        tap,
        skipped_counter(),
        (params.sps.clone(), params.pps.clone()),
    )
    .expect("open_with_sets must succeed");
    let video_path = session.output_path().to_path_buf();

    let tx = session.frame_sender();
    for n in 0..45 {
        tx.send(nbe_engine::record::thread::RecordMsg::Unit(fake_unit(
            n as f64 / 30.0,
            n == 0,
        )))
        .expect("frame channel must accept units");
    }
    let path = session
        .stop_and_finish(Duration::from_secs(10))
        .expect("stop on fed units must finish");
    assert_eq!(path, video_path);
    assert!(session.finished(), "finished flag must be set after stop");
    assert_moov_present(&video_path);
    let sidecar = nbe_engine::record::markers::sidecar_path(&video_path);
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).expect("sidecar must exist"))
            .expect("sidecar JSON must parse");
    assert_eq!(parsed["markers"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["markers"][0]["name"], "intro");
    // The take ended: the next take starts clean even with no new start yet.
    assert!(
        nbe_engine::record::markers::list().is_empty(),
        "thread finish must clear the marker store"
    );
}

/// `abandon` is the `force=true` immediate-stop path: flushed fragments stay
/// in the file as-is, no duration patch, no sidecar, take over.
#[test]
fn abandon_keeps_flushed_fragments_without_finish_or_sidecar() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let params = synthetic_params(dir.path());
    let tap = Arc::new(AudioTap::new());
    // Tone so fragments interleave AND flush mid-take: the writer holds a
    // video window until audio catches up (§9.3) — a silent take only
    // materializes at finish, exactly as a silent live take would.
    tap.push(&tone_secs(2));
    let mut session = session_glue::RecordSession::open_with_sets(
        &params.directory,
        &params.show,
        &params.episode,
        &params.start_timestamp,
        params.width,
        params.height,
        params.fps,
        tap,
        skipped_counter(),
        (params.sps.clone(), params.pps.clone()),
    )
    .expect("open_with_sets must succeed");
    let video_path = session.output_path().to_path_buf();

    // >1 s of video so at least one fragment flushes BEFORE any finish.
    let tx = session.frame_sender();
    for n in 0..45 {
        tx.send(nbe_engine::record::thread::RecordMsg::Unit(fake_unit(
            n as f64 / 30.0,
            n == 0,
        )))
        .expect("frame channel must accept units");
    }
    // Wait for the thread to flush a fragment (bounded: the thread is live).
    let mut moof = false;
    for _ in 0..200 {
        if let Ok(bytes) = std::fs::read(&video_path) {
            if top_level_boxes(&bytes).iter().any(|k| k == "moof") {
                moof = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(moof, "thread must flush ≥1 fragment before abandon");

    session.abandon();
    // Give the detached thread a moment to act, then check the file shape:
    // fragments kept, no finish artifacts.
    std::thread::sleep(Duration::from_millis(500));
    let top = top_level_boxes(&std::fs::read(&video_path).unwrap());
    assert!(top.iter().any(|k| k == "moof"), "abandon keeps fragments");
    assert!(
        !nbe_engine::record::markers::sidecar_path(&video_path).is_file(),
        "abandon writes no sidecar (no finish ran)"
    );
    assert!(session.finished(), "abandon ends the take");
}

/// Stopping with zero fed frames is loud (`E_RECORD_INPUT`): no video means
/// no recording, never an empty file masquerading as one.
#[test]
fn stop_with_no_frames_is_loud_and_leaves_no_file() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !aac_or_skip() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let tap = Arc::new(AudioTap::new());
    let mut session = session_glue::RecordSession::open(
        dir.path(),
        "demoshow",
        "ep01",
        "20260914T120000Z",
        640,
        360,
        30,
        tap,
        skipped_counter(),
    )
    .expect("open must succeed");
    let video_path: PathBuf = session.output_path().to_path_buf();
    let err = session
        .stop_and_finish(Duration::from_secs(10))
        .expect_err("empty take must refuse loudly");
    assert!(
        err.to_string().contains("E_RECORD_INPUT"),
        "expected E_RECORD_INPUT, got: {err}"
    );
    assert!(
        !video_path.is_file(),
        "no video means no recording file at all"
    );
}
