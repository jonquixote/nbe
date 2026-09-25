//! Gate G1 — who owns the surfaces when record and stream share one
//! composite (`agents/prompts/10-streaming.md` §2), on the production seams.
//!
//! One composite into one surface per frame, N consumers holding `Arc`s.
//! A consumer that falls behind gives its frame up (drops its `Arc`) rather
//! than holding the allocation hostage; the pool is sized for every
//! consumer's worst-case hold plus the one being drawn; record's
//! shed-before-draw is untouched.
//!
//! PR #30's first guard modelled this with `SharedPool<()>` — plain `Arc<()>`
//! slots under the free rule the real pool used — so it ran hermetically and
//! proved the model. It could not see that the real rule was wrong (a
//! surface VideoToolbox still reads looked free) or that the real sizing
//! left out each consumer's in-encode surface. These tests use what the loop
//! uses: the GPU `SurfacePool` from `shared_zerocopy_pool`, a real
//! `RenderLoop` drawing into it through `begin_tap_frame` / `restore_view` /
//! `end_tap_frame` (record) and `hand_off_stream_surface` (stream), and a
//! real VideoToolbox session for the free rule. The composition of these
//! seams inside `tick::run_tick` is exercised by `prompt10_rtmp`'s live
//! tests on a machine with an encoder.
//!
//! The CI runner has a zero-copy chain (Metal + IOSurface) and no encoder:
//! the pool guards run there; the VideoToolbox guard skips loudly.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use nbe_decode::zerocopy::{SharedSurface, SurfacePool};
use nbe_engine::record::pool::{shared_pool_size, stream_pool_size, IN_ENCODER};
use nbe_engine::record::stream::{hand_off_stream_surface, StreamMsg, STREAM_CHANNEL_BOUND};
use nbe_engine::record::{RecordMsg, RECORD_CHANNEL_BOUND};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::EngineState;

/// A render loop plus the shared pool, or a loud skip.
async fn chain_or_skip() -> Option<(Arc<EngineState>, RenderLoop, SurfacePool)> {
    let state = Arc::new(EngineState::new(30));
    let render = RenderLoop::new(state.clone()).await.ok()?;
    let Some(device) = state.render_device() else {
        eprintln!("SKIP: no render device on this machine");
        return None;
    };
    match nbe_engine::record::shared_zerocopy_pool(&device) {
        Ok(pool) => Some((state, render, pool)),
        Err(e) => {
            eprintln!("SKIP: no zero-copy chain on this machine ({e})");
            None
        }
    }
}

/// The sizing rule, pinned against the bounds it is built from.
#[test]
fn sizing_counts_each_consumers_queue_and_encoder_plus_the_drawn_one() {
    assert_eq!(IN_ENCODER, 1);
    assert_eq!(
        shared_pool_size(RECORD_CHANNEL_BOUND, STREAM_CHANNEL_BOUND),
        (RECORD_CHANNEL_BOUND + 1) + (STREAM_CHANNEL_BOUND + 1) + 1
    );
    assert_eq!(
        stream_pool_size(STREAM_CHANNEL_BOUND),
        STREAM_CHANNEL_BOUND + 1 + 1
    );
}

/// Worst case, on real surfaces: record holds its whole queue plus the one
/// it is encoding, a stalled stream holds ITS whole queue plus its
/// in-encode surface — on frames disjoint from record's, which is possible
/// because either consumer may shed a frame the other keeps — and the next
/// draw still gets a surface.
///
/// Falsifies the sizing fix: PR #30's `record bound + stream bound + 1` (5)
/// runs out here.
#[tokio::test]
async fn shared_pool_fits_both_consumers_worst_case_plus_the_draw() {
    let Some((_state, _render, pool)) = chain_or_skip().await else {
        return;
    };
    // Written out, not derived from `IN_ENCODER`: the constant is what is
    // under test. Each consumer: its full queue plus the one in its encoder.
    let mut held: Vec<Arc<SharedSurface>> = Vec::new();
    for _ in 0..(RECORD_CHANNEL_BOUND + 1) {
        held.push(pool.acquire().expect("record's worst-case hold fits"));
    }
    for _ in 0..(STREAM_CHANNEL_BOUND + 1) {
        held.push(
            pool.acquire()
                .expect("a stalled stream's worst-case hold fits"),
        );
    }
    assert!(
        pool.acquire().is_some(),
        "with both consumers at their worst, the compositor still has a surface to draw into \
         (pool of {})",
        pool.len()
    );
}

/// The record thread at its worst, driven deterministically: every frame
/// handed off is received into a local queue, and only when that queue
/// exceeds the channel bound does the oldest move into the "encoder" (whose
/// previous frame is then done and dropped). Record therefore holds its full
/// queue plus one in its encoder at every tick — the case the sizing must
/// fit — without its channel ever shedding (which would be a record skip of
/// record's own making, not the stream's).
struct RecordAtItsBound {
    rx: Receiver<RecordMsg>,
    queue: std::collections::VecDeque<Arc<SharedSurface>>,
    encoding: Option<Arc<SharedSurface>>,
    encoded: u64,
}

impl RecordAtItsBound {
    fn tick(&mut self) {
        while let Ok(RecordMsg::Surface { surface }) = self.rx.try_recv() {
            self.queue.push_back(surface);
        }
        while self.queue.len() > RECORD_CHANNEL_BOUND {
            self.encoding = self.queue.pop_front();
            self.encoded += 1;
        }
    }
}

/// A stalled stream thread: takes ONE surface into its "encoder" and never
/// finishes it, and never reads its channel again — so the channel fills and
/// every later handoff is shed. Holds everything until `STOP`.
fn stalled_stream(rx: Receiver<StreamMsg>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let in_encoder = rx.recv();
        while !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        drop((in_encoder, rx));
    })
}

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// THE GUARD: a stalled stream costs record nothing. Sixty frames drawn into
/// the shared pool by the real render loop through record's production
/// seams, each shared with a stream whose thread has stalled: the stream's
/// sheds are counted on the stream counter; record never skips a frame; the
/// stream handoff never waits.
///
/// Falsified by (a) shrinking the pool to PR #30's size — record skips rise
/// — and (b) a handoff that waits for room instead of shedding — the tick
/// blocks on the stalled stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_stream_costs_record_nothing() {
    let Some((state, mut render, pool)) = chain_or_skip().await else {
        return;
    };
    STOP.store(false, Ordering::SeqCst);
    let (record_tx, record_rx): (SyncSender<RecordMsg>, _) =
        std::sync::mpsc::sync_channel(RECORD_CHANNEL_BOUND);
    let (stream_tx, stream_rx) = std::sync::mpsc::sync_channel(STREAM_CHANNEL_BOUND);
    let mut record = RecordAtItsBound {
        rx: record_rx,
        queue: Default::default(),
        encoding: None,
        encoded: 0,
    };
    let stream = stalled_stream(stream_rx);
    let record_skips = AtomicU64::new(0);
    let stream_skips = AtomicU64::new(0);
    let mut worst_handoff = Duration::ZERO;

    for frame in 0..60u64 {
        let loan =
            nbe_engine::record::begin_tap_frame(&mut render, Some(&pool), true, &record_skips)
                .expect("the pool exists: no chain loss");
        let started = std::time::Instant::now();
        let _ = render.render_frame(frame, None);
        let render_elapsed = started.elapsed();
        nbe_engine::record::restore_view(&mut render, &loan);
        let shared = loan.surface();
        let _ = nbe_engine::record::end_tap_frame(
            loan,
            render_elapsed,
            None,
            &record_tx,
            &record_skips,
            // A zero-copy take never reads back; the closure is never called.
            || async { (Vec::new(), Duration::ZERO) },
        )
        .await;
        if let Some(surface) = shared {
            let t = std::time::Instant::now();
            hand_off_stream_surface(&stream_tx, surface, frame, &stream_skips);
            worst_handoff = worst_handoff.max(t.elapsed());
        }
        record.tick();
    }
    let (r, s) = (
        record_skips.load(Ordering::SeqCst),
        stream_skips.load(Ordering::SeqCst),
    );
    drop(record_tx);
    let recorded = record.encoded;
    STOP.store(true, Ordering::SeqCst);
    stream.join().unwrap();
    drop(stream_tx);
    println!(
        "G1 guard: pool {} surfaces, 60 frames: record encoded {recorded}, record skips {r}, \
         stream sheds {s}, worst stream handoff {worst_handoff:?}, View drops {}",
        pool.len(),
        state.dropped_frames_total.load(Ordering::SeqCst)
    );
    assert_eq!(r, 0, "a stalled stream must never cost record a frame");
    assert!(
        s >= 55,
        "the stalled stream sheds, counted on its own counter (got {s})"
    );
    // A handoff that waited for room would block forever behind this stalled
    // consumer, so any finite bound discriminates; 20 ms leaves room for
    // scheduler preemption on a shared CI runner.
    assert!(
        worst_handoff < Duration::from_millis(20),
        "the stream handoff never waits ({worst_handoff:?})"
    );
    assert_eq!(state.dropped_frames_total.load(Ordering::SeqCst), 0);
}

/// The free rule waits for VideoToolbox, not just for Rust.
///
/// `VTCompressionSessionEncodeFrame` retains the `CVPixelBuffer` and encodes
/// asynchronously: after `encode_pixel_buffer` returns, the buffer's retain
/// count reads 2 until the encoder finishes (measured 1.6–17.5 ms). The
/// record thread — merged in ZERO-COPY Phase 3b — drops its `Arc` the moment
/// the call returns, and the pool's old rule (`Arc::strong_count == 1`) then
/// called the surface free while the encoder was still reading it: the
/// compositor could draw the next frame into pixels being encoded.
///
/// Falsified by reverting `SurfacePool`'s rule to the count alone: the pool
/// hands back a surface VideoToolbox still holds.
#[tokio::test]
async fn the_pool_never_hands_out_a_surface_videotoolbox_still_reads() {
    if !nbe_engine::record::encoder_available() {
        eprintln!("SKIP: no hardware H.264 encoder on this machine (SPEC §9.2)");
        return;
    }
    let Some((_state, _render, pool)) = chain_or_skip().await else {
        return;
    };
    let (w, h) = pool.dimensions();
    let mut enc = nbe_decode::encode::EncodeSession::open(w, h, 30, 8_000_000)
        .expect("the encoder opens at pool geometry");
    let mut observed_held = 0;
    let mut handed_out_while_held = 0;
    for _ in 0..12 {
        let surface = pool.acquire().expect("a free surface");
        let id = surface.surface_id();
        let _ = enc.encode_pixel_buffer(surface.pixel_buffer());
        // VideoToolbox's hold, observed directly (not inferred from the pool).
        let vt_held = !surface.encoder_released();
        // Exactly what the record thread does next.
        drop(surface);
        // Ask for every free surface while VideoToolbox may still hold ours.
        let mut taken = Vec::new();
        while let Some(s) = pool.acquire() {
            taken.push(s);
        }
        if vt_held {
            observed_held += 1;
            // A hand-out is a violation only if VideoToolbox STILL holds the
            // buffer now that the compositor has it.
            if let Some(s) = taken.iter().find(|s| s.surface_id() == id) {
                if !s.encoder_released() {
                    handed_out_while_held += 1;
                }
            }
        }
        drop(taken);
        std::thread::sleep(Duration::from_millis(40)); // let VT finish before the next round
    }
    println!(
        "VT retain guard: 12 encodes; VideoToolbox still held the buffer when the call \
         returned {observed_held} times; the pool handed it out while held \
         {handed_out_while_held} times"
    );
    assert!(
        observed_held > 0,
        "VideoToolbox released every buffer synchronously: the hazard was not observed, so \
         this run proves nothing — investigate before trusting it"
    );
    assert_eq!(
        handed_out_while_held, 0,
        "the pool must never hand out a surface VideoToolbox still reads"
    );
}
