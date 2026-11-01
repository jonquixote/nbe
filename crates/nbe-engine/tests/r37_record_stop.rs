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
//! any machine: the runtime's one blocking thread is held until the stand-in
//! has run with the stop under way, so a finish quick enough to beat the first
//! poll cannot read as a stop that held the thread (R39). Hardware-free: the
//! session is the synthetic-unit seam
//! (`RecordSession::open_with_sets`), whose finish runs the real writer
//! (AudioToolbox AAC).
//!
//! The loop drawing during the finish opens a window R37's first form missed:
//! the take is still `Recording` and its session is already gone. A tick
//! there read a claimed zero-copy take with no pool as a lost chain, and ended
//! the take with a false `E_NO_ZEROCOPY` error on every ordinary stop (and
//! on `show.stop`, whose finish has had this window since it moved to the
//! blocking pool). The two tick tests below pin that a take with no session is
//! not a lost chain, and that a take which keeps its session and loses its
//! pool still is. They read the real `run_tick` on a real `RenderLoop`, in a
//! window set by hand. A fourth runs the real `record.stop` beside the real
//! `run_loop` on one thread, the finish held behind the pool so the window is
//! there to draw in.

use std::cell::Cell;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::record::session as session_glue;
use nbe_engine::record::tap_path::{Reason, Selection, TapPath};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, OutgoingQueue, RecordState};
use nbe_engine::tick::{run_loop, run_tick};
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

/// One thread, as in `main`, and one blocking thread for [`hold_the_pool`].
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .expect("a test runtime")
}

/// Hold the blocking pool's one thread until the returned sender is dropped: a
/// finish queued now cannot start before the test lets it. The bound turns a
/// hang into a red and measures nothing.
fn hold_the_pool() -> mpsc::Sender<()> {
    let (release, held) = mpsc::channel::<()>();
    tokio::task::spawn_blocking(move || {
        let _ = held.recv_timeout(Duration::from_secs(30));
    });
    release
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

/// A synthetic take through the real record thread, `secs` long, open and
/// recording on `state`. Returns where its file will land.
fn open_a_take(state: &EngineState, dir: &Path, secs: u32) -> std::path::PathBuf {
    let tap = Arc::new(nbe_engine::record::AudioTap::new());
    tap.push(&tone_secs(secs));
    let session = session_glue::RecordSession::open_with_sets(
        dir,
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
    for n in 0..secs * 30 {
        tx.send(nbe_engine::record::thread::RecordMsg::Unit(fake_unit(
            f64::from(n) / 30.0,
            n % 30 == 0,
        )))
        .expect("the record thread must accept units");
    }
    video_path
}

#[test]
fn record_stop_frees_the_thread_while_the_take_finishes_and_acks_after() {
    runtime().block_on(stop_beside_the_stand_in());
}

async fn stop_beside_the_stand_in() {
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
    let video_path = open_a_take(&state, dir.path(), 10);

    // The directive path and the loop's stand-in share one thread, as in
    // `main`: the stand-in only counts its own polls.
    let local = tokio::task::LocalSet::new();
    let (during, stop_ms, result) = local
        .run_until(async {
            let polls = Rc::new(Cell::new(0u64));
            let running = Rc::new(Cell::new(true));
            let stopping = Rc::new(Cell::new(false));
            let (p, r, s) = (polls.clone(), running.clone(), stopping.clone());
            let mut held = Some(hold_the_pool());
            let stand_in = tokio::task::spawn_local(async move {
                while r.get() {
                    p.set(p.get() + 1);
                    if s.get() {
                        drop(held.take());
                    }
                    tokio::task::yield_now().await;
                }
            });
            tokio::task::yield_now().await;
            let before = polls.get();
            stopping.set(true);
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

/// Log lines captured from the tick's thread.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The engine state and its render loop, a take that claims zero-copy, and the
/// captured log; or a loud skip without an adapter.
async fn zero_copy_take_or_skip() -> Option<(Arc<EngineState>, RenderLoop, Captured)> {
    let state = Arc::new(EngineState::new(30));
    let render = match RenderLoop::new(state.clone()).await {
        Ok(render) => render,
        Err(e) => {
            eprintln!("SKIP: no GPU adapter for the render loop ({e})");
            return None;
        }
    };
    *state.record_tap_selection.lock().unwrap() = Some(Selection {
        path: TapPath::ZeroCopy,
        reason: Reason::Table,
    });
    *state.record_state.lock().unwrap() = RecordState::Recording;
    Some((state, render, Captured::default()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tick_inside_a_stop_s_finish_is_a_take_ending_not_a_lost_chain() {
    let Some((state, mut render, captured)) = zero_copy_take_or_skip().await else {
        return;
    };
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // The window: `Recording`, the claim standing, the session already taken.
    assert!(state.record_session.lock().unwrap().is_none());
    run_tick(&mut render, &state, 0, None).await;

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        !log.contains("chain lost"),
        "a stop's finish was reported as a lost chain: {log}"
    );
    assert_eq!(
        *state.record_state.lock().unwrap(),
        RecordState::Recording,
        "the stop ends the take, once its file is complete; the tick does not"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_take_that_keeps_its_session_and_loses_its_pool_still_ends_loudly() {
    let Some((state, mut render, captured)) = zero_copy_take_or_skip().await else {
        return;
    };
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // Option A's case: the session stands and its pool is gone.
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let session = session_glue::RecordSession::open_with_sets(
        dir.path(),
        "demoshow",
        "ep01",
        "20261029T120000Z",
        640,
        360,
        30,
        Arc::new(nbe_engine::record::AudioTap::new()),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        (vec![0x67, 0x64, 0x00, 0x1E, 0xAA], vec![0x68, 0x11, 0x22]),
    )
    .expect("test-seam open must succeed");
    *state.record_session.lock().unwrap() = Some(session);
    run_tick(&mut render, &state, 0, None).await;

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("chain lost"), "{log}");
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
}

/// R37's first form end to end, as `main` runs it: the directive path and the
/// render loop on one thread, a real `record.stop` and the real `run_loop`, on
/// a take that claims zero-copy. The finish is held behind the pool so the loop
/// draws in the window, and the guard reads that it did.
#[test]
fn a_real_stop_beside_the_real_loop_is_a_take_ending_not_a_lost_chain() {
    runtime().block_on(stop_beside_the_loop());
}

async fn stop_beside_the_loop() {
    if !aac_or_skip() {
        return;
    }
    let Some((state, mut render, captured)) = zero_copy_take_or_skip().await else {
        return;
    };
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    nbe_engine::record::markers::clear();
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    handler.apply(&directive("show.start", 1)).await.unwrap();
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let video_path = open_a_take(&state, dir.path(), 1);

    let local = tokio::task::LocalSet::new();
    let (inside, result) = local
        .run_until(async {
            let mut held = Some(hold_the_pool());
            let stop = tokio::task::spawn_local(async move {
                handler.apply(&directive("record.stop", 2)).await
            });
            // Nothing ticks until the stop has taken its session: a tick
            // before that sees a take with a session and no pool, which is a
            // lost chain, rightly.
            while state.record_session.lock().unwrap().is_some() {
                tokio::task::yield_now().await;
            }
            let mut inside = 0;
            run_loop(&mut render, &state, 30, |_| {
                let recording = *state.record_state.lock().unwrap() == RecordState::Recording;
                inside += u32::from(recording);
                if inside == 3 || !recording {
                    drop(held.take());
                }
                !stop.is_finished()
            })
            .await;
            (inside, stop.await.expect("the stop must not panic"))
        })
        .await;
    result.expect("record.stop must finish the synthetic take");

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        !log.contains("chain lost"),
        "a real stop's finish was reported as a lost chain: {log}"
    );
    assert!(
        inside >= 3,
        "the loop drew {inside} ticks inside the stop's finish"
    );
    assert!(has_moov(&video_path), "the file must be complete");
    assert_eq!(*state.record_state.lock().unwrap(), RecordState::Idle);
    assert!(acked(&outgoing, 2), "the stop's ack must follow its finish");
    nbe_engine::record::markers::clear();
}
