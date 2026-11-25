//! One GPU poller (the user's direction of 2026-11-24, item 1). A consumer
//! waiting for a View draw (`DrawDone::wait`, as the record and stream threads
//! do) takes no wgpu-core lock while the render thread draws. The render
//! thread's own once-a-frame poll fires the draw's completion flag.
//!
//! The defect: the waiter polled the device every 500 µs, taking
//! `Device::command_indices` and `Queue::life_tracker`. On CI the lock probe
//! caught the render thread waiting 13.0 and 20.4 ms behind `nbe-record`
//! (NBE_LOCK_TIMING, main d9158fc's dress).
//!
//! Its own binary: the probe reads `NBE_LOCK_TIMING` once, at the first
//! wgpu-core lock in the process.

use std::sync::Mutex;
use std::time::Duration;

use nbe_engine::record::{AwaitDraw, DrawDone, DRAW_WAIT, RENDER_POLL_STALE};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::EngineState;

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
struct Capture;
impl log::Log for Capture {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target() == "nbe_lock"
    }
    fn log(&self, r: &log::Record) {
        if r.target() == "nbe_lock" {
            LINES.lock().unwrap().push(r.args().to_string());
        }
    }
    fn flush(&self) {}
}
static CAPTURE: Capture = Capture;

/// Render 60 frames at `cadence` while a thread named `nbe-record` waits on
/// each draw, then one more frame (its next frame's poll); return the
/// probe's reports that name the consumer.
fn consumer_lock_reports(render: &mut RenderLoop, cadence: Duration) -> (Vec<String>, Duration) {
    LINES.lock().unwrap().clear();
    let (tx, rx) = std::sync::mpsc::channel::<Option<DrawDone>>();
    // The record thread's name, as the engine's own record thread carries it.
    let consumer = std::thread::Builder::new()
        .name("nbe-record".into())
        .spawn(move || {
            let mut waited = 0;
            while let Ok(Some(drawn)) = rx.recv() {
                assert!(drawn.wait(DRAW_WAIT), "every draw is seen done");
                waited += 1;
            }
            waited
        })
        .unwrap();
    // Paced to the interval, as `run_loop` paces to the frame boundary: a
    // sleep after each draw would add the draw's own time (93 ms a frame on
    // #225's second CI run) to every interval.
    let mut next = std::time::Instant::now();
    // The render thread's longest silence: the gap between two frames'
    // starts, where it polls. A waiter may poll the device itself only after
    // one of at least RENDER_POLL_STALE (a CI VM freeze: #225's third run
    // saw the render thread silent 603 ms, and the waiter then polled).
    let mut last_start: Option<std::time::Instant> = None;
    let mut longest = Duration::ZERO;
    let mut start = |longest: &mut Duration| {
        let now = std::time::Instant::now();
        if let Some(prev) = last_start {
            *longest = (*longest).max(now - prev);
        }
        last_start = Some(now);
    };
    for frame in 0..60u64 {
        start(&mut longest);
        render.render_frame(frame, None);
        let drawn = DrawDone::of_view(render).expect("the View drew");
        tx.send(Some(drawn)).unwrap();
        next += cadence;
        if let Some(rest) = next.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(rest);
        }
    }
    // The production loop never stops drawing, so neither does this one
    // until the consumer has seen every draw done. A render thread that went
    // silent here made the consumer's last wait fall back to its own fence
    // check, by design: one poll, 17 acquires (#231's run at 33 ms, 2026-11-25
    // 05:19Z, a slow GPU still on frame 59 at frame 60's poll).
    tx.send(None).unwrap();
    let mut frame = 60u64;
    while !consumer.is_finished() {
        assert!(
            frame < 600,
            "the consumer did not finish within 540 more frames"
        );
        start(&mut longest);
        render.render_frame(frame, None);
        frame += 1;
        next += cadence;
        if let Some(rest) = next.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(rest);
        }
    }
    assert_eq!(
        consumer.join().unwrap(),
        60,
        "the consumer saw every draw done"
    );
    let lines = LINES.lock().unwrap().clone();
    assert!(
        !lines.is_empty(),
        "the probe reported the render thread's own acquires"
    );
    let reports = lines
        .into_iter()
        .filter(|l| l.contains("nbe-record"))
        .collect();
    (reports, longest)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_consumer_waiting_for_the_view_draw_takes_no_wgpu_lock_while_the_render_thread_draws() {
    std::env::set_var("NBE_LOCK_TIMING", "1");
    std::env::set_var("NBE_LOCK_TIMING_MS", "0"); // every acquire reports
    log::set_logger(&CAPTURE).unwrap();
    log::set_max_level(log::LevelFilter::Warn);

    let state = std::sync::Arc::new(EngineState::new(30));
    let mut render = RenderLoop::new(state)
        .await
        .expect("the one-poller test needs a wgpu adapter");
    // At the house rate, and at a starved cadence slower than any fixed wait
    // on the flag would cover: #225's first CI run fell back to polling
    // there (116 acquires by `nbe-record`). 150 ms is still under
    // RENDER_POLL_STALE: the render thread is drawing, slowly.
    for cadence in [Duration::from_millis(33), Duration::from_millis(150)] {
        let (reports, longest) = consumer_lock_reports(&mut render, cadence);
        assert!(
            reports.is_empty() || longest >= RENDER_POLL_STALE,
            "at {cadence:?} a frame the consumer took wgpu-core locks ({} reports) although the \
             render thread was never silent for {RENDER_POLL_STALE:?} (its longest gap: \
             {longest:?}), so it polled the device beside a drawing render thread: {:?}",
            reports.len(),
            &reports[..reports.len().min(3)]
        );
        if !reports.is_empty() {
            eprintln!(
                "at {cadence:?}: {} acquires by the consumer, after a render-thread silence of {longest:?} (allowed)",
                reports.len()
            );
        }
    }
}
