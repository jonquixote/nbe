//! R37 (filed 2026-10-28): `record.stop` waited on the render loop's own
//! thread while the record thread finished the file. `main` drives the
//! directive path and the render loop on one thread (one `LocalSet`), so a
//! stop after a long take held that thread 55–65 ms and a master frame was
//! never drawn. The finish now runs on the blocking pool and the stop awaits
//! it: the thread is free while the file is written, and the ack still
//! follows the finish.
//!
//! The guard runs `record.stop` beside a task on the same thread that only
//! counts its own polls, the render loop's stand-in. A stop that holds the
//! thread completes inside one poll, so the count cannot move while it runs;
//! a stop that yields lets it move. No timing is read, so the guard holds on
//! any machine. Hardware-free: the session is the synthetic-unit seam
//! (`RecordSession::open_with_sets`), whose finish runs the real writer
//! (AudioToolbox AAC).

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::record::session as session_glue;
use nbe_engine::state::{EngineState, OutgoingQueue, RecordState};
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION};

fn directive(command: &str, sv: u64) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target: serde_json::json!({}),
        payload: serde_json::json!({}),
    }
}

fn aac_or_skip() -> bool {
    if nbe_engine::record::aac::is_available() {
        true
    } else {
        eprintln!("SKIP: AudioToolbox AAC absent — the synthetic take's finish needs the encoder");
        false
    }
}

fn acked(outgoing: &OutgoingQueue, sv: u64) -> bool {
    outgoing.drain().into_iter().any(|f| match f {
        EngineFrame::AppliedStateVersion { state_version, .. } => state_version == sv,
        _ => false,
    })
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

/// The file is complete: `ftyp` first and a `moov` among its top-level boxes.
fn has_moov(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let (mut off, mut top) = (0usize, Vec::new());
    while off + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let len = if len == 1 {
            u64::from_be_bytes(bytes[off + 8..off + 16].try_into().unwrap()) as usize
        } else {
            len
        };
        if len < 8 {
            return false;
        }
        top.push(bytes[off + 4..off + 8].to_vec());
        off += len;
    }
    top.first().map(Vec::as_slice) == Some(b"ftyp") && top.iter().any(|k| k == b"moov")
}

#[tokio::test(flavor = "current_thread")]
async fn record_stop_frees_the_thread_while_the_take_finishes_and_acks_after() {
    if !aac_or_skip() {
        return;
    }
    nbe_engine::record::markers::clear();
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    handler.apply(&directive("show.start", 1)).await.unwrap();

    // A ten-second synthetic take through the real record thread.
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let tap = Arc::new(nbe_engine::record::AudioTap::new());
    tap.push(&tone_secs(10));
    let session = session_glue::RecordSession::open_with_sets(
        dir.path(),
        "demoshow",
        "ep01",
        "20261029T120000Z",
        640,
        360,
        30,
        tap.clone(),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        (vec![0x67, 0x64, 0x00, 0x1E, 0xAA], vec![0x68, 0x11, 0x22]),
    )
    .expect("test-seam open must succeed where AAC exists");
    let video_path = session.output_path().to_path_buf();
    let tx = session.frame_sender();
    *state.record_tap.lock().unwrap() = Some(tap);
    *state.record_session.lock().unwrap() = Some(session);
    *state.record_state.lock().unwrap() = RecordState::Recording;
    for n in 0..300 {
        tx.send(nbe_engine::record::thread::RecordMsg::Unit(fake_unit(
            n as f64 / 30.0,
            n % 30 == 0,
        )))
        .expect("the record thread must accept units");
    }
    drop(tx);

    // The directive path and the loop's stand-in share one thread, as in
    // `main`: the stand-in only counts its own polls.
    let local = tokio::task::LocalSet::new();
    let (during, stop_ms, result) = local
        .run_until(async {
            let polls = Rc::new(Cell::new(0u64));
            let running = Rc::new(Cell::new(true));
            let (p, r) = (polls.clone(), running.clone());
            let stand_in = tokio::task::spawn_local(async move {
                while r.get() {
                    p.set(p.get() + 1);
                    tokio::task::yield_now().await;
                }
            });
            tokio::task::yield_now().await;
            let before = polls.get();
            let t0 = Instant::now();
            let result = handler.apply(&directive("record.stop", 2)).await;
            let stop_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let during = polls.get() - before;
            running.set(false);
            stand_in.await.expect("the stand-in must end cleanly");
            (during, stop_ms, result)
        })
        .await;
    result.expect("record.stop must finish the synthetic take");
    println!(
        "R37: the loop's stand-in ran {during} polls while record.stop finished the take ({stop_ms:.1} ms)"
    );
    assert!(
        during > 0,
        "record.stop held the thread for its whole finish ({stop_ms:.1} ms): the loop's stand-in never ran (R37)"
    );

    // The finalisation semantics are unchanged: when the stop returns, the
    // file and its sidecar are complete, the take is over, and the ack is out.
    assert!(
        has_moov(&video_path),
        "the file must be complete (ftyp, moov) when record.stop returns"
    );
    assert!(
        nbe_engine::record::markers::sidecar_path(&video_path).is_file(),
        "the sidecar must be written when record.stop returns"
    );
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(state.record_session.lock().unwrap().is_none());
    assert_eq!(state.last_applied(), 2);
    assert!(acked(&outgoing, 2), "the stop's ack must follow its finish");
    nbe_engine::record::markers::clear();
}
