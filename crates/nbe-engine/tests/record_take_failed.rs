//! A take whose record thread ends without a stop reads `failed` on the tick.
//!
//! Before: a writer error ends the record thread (`drain_tap_fatal`, or the
//! file cannot be created), and so does a panic, but nothing moved: the render
//! loop's hand-off saw a disconnected channel and counted a shed, the engine's
//! state stayed `Recording`, and the telemetry tick carried no record state at
//! all. The operator learned the take was gone at `record.stop`.
//!
//! Hermetic: the `Unit` seam with caller-supplied parameter sets, no encoder.
//! Before the take's first content, its directory is replaced by a regular
//! file of the same name, so the writer (which creates the directory and the
//! file on first content) cannot: a real failure, not an injected one.

use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::record::session::RecordSession;
use nbe_engine::record::thread::RecordMsg;
use nbe_engine::record::AudioTap;
use nbe_engine::state::{EngineState, RecordState};
use nbe_engine::telemetry::record_take_line;
use nbe_protocol::RecordTakeLine;

/// The record tests share process-wide seams (the marker store).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn open(dir: &Path) -> RecordSession {
    RecordSession::open_with_sets(
        dir,
        "failshow",
        "ep01",
        "20261121T000000Z",
        640,
        360,
        30,
        Arc::new(AudioTap::new()),
        Arc::new(AtomicU64::new(0)),
        (vec![0x67, 0x64, 0x00, 0x1E, 0xAA], vec![0x68, 0x11, 0x22]),
    )
    .expect("the take opens")
}

fn keyframe() -> nbe_engine::encode::EncodedUnit {
    nbe_engine::encode::EncodedUnit {
        data: vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x11, 0x22, 0x33],
        pts_seconds: 0.0,
        is_keyframe: true,
    }
}

/// An engine state holding `session` as its running take.
fn recording(session: RecordSession) -> EngineState {
    let state = EngineState::new(30);
    *state.record_state.lock().unwrap() = RecordState::Recording;
    *state.record_session.lock().unwrap() = Some(session);
    state
}

#[test]
fn a_take_whose_file_cannot_be_written_reads_failed_on_the_tick() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("takes");
    let session = open(&dir);
    assert_eq!(session.ended_on_its_own(), None, "a fresh take runs");
    std::fs::remove_dir_all(&dir).expect("remove the take's directory");
    std::fs::write(&dir, b"not a directory").expect("a file where the directory was");
    session
        .frame_sender()
        .send(RecordMsg::Unit(keyframe()))
        .expect("the thread takes the unit");

    let state = recording(session);
    let waited = Instant::now();
    let line = loop {
        let line = record_take_line(&state);
        if line.state != "recording" || waited.elapsed() > Duration::from_secs(5) {
            break line;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    println!("after {:?}: {line:?}", waited.elapsed());
    assert_eq!(
        line.state, "failed",
        "the tick says the take failed: {line:?}"
    );
    let reason = line.reason.expect("a failed take says why");
    assert!(!reason.is_empty());

    // The stop still ends the take, and reports the failure rather than hang.
    let mut session = state.record_session.lock().unwrap().take().expect("held");
    *state.record_state.lock().unwrap() = RecordState::Idle;
    assert!(
        session.stop_and_finish(Duration::from_secs(5)).is_err(),
        "the stop of a failed take reports the failure"
    );
    assert_eq!(record_take_line(&state), RecordTakeLine::none());
}

#[test]
fn a_running_take_reads_recording_and_a_stopped_one_never_reads_failed() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().expect("tempdir");
    let state = EngineState::new(30);
    assert_eq!(record_take_line(&state), RecordTakeLine::none(), "no take");

    let state = recording(open(root.path()));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(record_take_line(&state), RecordTakeLine::recording());

    // A stop is an asked-for end: whatever the finish reports, the take did
    // not end on its own.
    let mut session = state.record_session.lock().unwrap().take().expect("held");
    let _ = session.stop_and_finish(Duration::from_secs(5));
    assert_eq!(
        session.ended_on_its_own(),
        None,
        "a stopped take never reads as one that ended on its own"
    );
}
