//! One render-loop tick: draw the View, then hand the frame to the outputs.
//!
//! The loop ([`run_loop`]) owns the cadence — sleep to the frame boundary,
//! read the master frame, call [`run_tick`], advance. Both live here, in the
//! library: the binary calls `run_loop` and never returns, tests and
//! measurements call it with a stop condition — so they drive the
//! production loop itself (§2a rule 7). PR #30's first stream leg lived in
//! `main.rs` as `feed_stream_leg`, where no test could reach it, and its
//! "View never drops" claims were checked against harnesses that imitated it.
//!
//! ## Output work per tick
//!
//! * **Record** (unchanged from Prompt 09 / ZERO-COPY Phase 3b): before the
//!   draw, the take's pool loans a surface ([`begin_tap_frame`]); after it,
//!   the budget pre-check then the bounded handoff ([`end_tap_frame`]). Cost
//!   accumulates into `record_tap_ms`, skips into `skipped_record_frames`.
//! * **Stream**: before the draw, if the record loan holds no surface, the
//!   stream's own pool loans one (G1: when the record loan holds one, the
//!   stream shares that composite — one draw, N holders). After the draw,
//!   one bounded `try_send` of the `Arc` to the stream thread
//!   ([`hand_off_stream_surface`]) — the loop never encodes, never opens an
//!   encoder, never waits. Cost accumulates into `stream_tap_ms`, drops into
//!   `skipped_stream_frames`; neither ever touches record skips or View drops.
//!
//! [`begin_tap_frame`]: crate::record::begin_tap_frame
//! [`end_tap_frame`]: crate::record::end_tap_frame
//! [`hand_off_stream_surface`]: crate::record::stream::hand_off_stream_surface

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::render::RenderLoop;
use crate::state::{EngineState, RecordState, StreamState};

/// Where one tick's time went. `total` is the loop's timed region (the tick,
/// without the sleep to the next boundary).
#[derive(Debug, Clone, Copy, Default)]
pub struct TickReport {
    pub render: Duration,
    /// Record handoff after the draw (the readback lives here on a CPU take).
    pub record: Duration,
    /// Stream work: the pre-draw surface loan plus the post-draw `try_send`.
    pub stream: Duration,
    pub total: Duration,
    /// The stream handed this frame to its thread.
    pub stream_sent: bool,
}

/// Draw frame `frame` and feed the live outputs. See the module docs.
pub async fn run_tick(
    render: &mut RenderLoop,
    state: &EngineState,
    frame: u64,
    deadline: Option<Duration>,
) -> TickReport {
    let tick_started = Instant::now();
    let mut report = TickReport::default();

    // One lock acquisition resolves the take's pool and handoff endpoint, so
    // they cannot describe different takes.
    let recording = *state.record_state.lock().unwrap() == RecordState::Recording;
    let (pool, endpoint) = match recording {
        true => {
            let g = state.record_session.lock().unwrap();
            match g.as_ref() {
                Some(s) => (s.surface_pool(), Some(s.frame_sender())),
                None => (None, None),
            }
        }
        false => (None, None),
    };
    // What the take CLAIMS, which is what makes a missing pool a chain loss
    // rather than an ordinary CPU take.
    let claims_zero_copy = matches!(
        *state.record_tap_selection.lock().unwrap(),
        Some(sel) if sel.path == crate::record::tap_path::TapPath::ZeroCopy
    ) && recording;
    let loan = match crate::record::begin_tap_frame(
        render,
        pool.as_deref(),
        claims_zero_copy,
        &state.skipped_record_frames,
    ) {
        Ok(loan) => loan,
        // Option A: the take ends here, loudly. The View still goes on air.
        Err(e) => {
            crate::record::end_take_on_chain_loss(state, &e.to_string());
            Default::default()
        }
    };

    // Stream, before the draw. One short lock scope clones the session's
    // endpoints; nothing below holds a session lock.
    let stream_started = Instant::now();
    let streaming = *state.stream_state.lock().unwrap() == StreamState::Live;
    let (stream_tx, stream_pool) = match streaming {
        true => {
            let g = state.stream_session.lock().unwrap();
            match g.as_ref() {
                Some(s) => (s.frame_sender(), s.surface_pool()),
                None => (None, None),
            }
        }
        false => (None, None),
    };
    let mut own_surface = None;
    if stream_tx.is_some() && loan.surface().is_none() {
        // No record surface to share (stream-only, a CPU take, or a take that
        // skipped pre-draw): the stream's own pool. A busy pool (every
        // surface queued or still held by VideoToolbox) is a stream drop,
        // counted — the View draws to its built-in target regardless.
        match stream_pool.as_ref().and_then(|p| p.acquire()) {
            Some(s) if render.set_view_surface(Some(s.clone())).is_ok() => {
                own_surface = Some(s);
            }
            _ => {
                state.skipped_stream_frames.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    report.stream += stream_started.elapsed();

    let render_started = Instant::now();
    let _ = render.render_frame(frame, deadline);
    report.render = render_started.elapsed();
    // Back to the built-in target the moment the draw is done: a retarget
    // left in place would composite the NEXT frame into a surface nobody
    // is holding.
    crate::record::restore_view(render, &loan);
    if own_surface.is_some() {
        let _ = render.set_view_surface(None);
    }
    // Both-live share: clone the record loan's surface BEFORE the record
    // branch moves the loan (read-only; record's path is untouched).
    let shared_surface = loan.surface();

    if recording {
        let record_started = Instant::now();
        if let Some(tx) = endpoint {
            let feed_ms = crate::record::end_tap_frame(
                loan,
                report.render,
                deadline,
                &tx,
                &state.skipped_record_frames,
                || async {
                    // Timed readback (the only await, and only on the CPU
                    // path): its cost belongs to the record counter.
                    let started = Instant::now();
                    let rgba = render.readback_view().await;
                    (rgba, started.elapsed())
                },
            )
            .await;
            *state.record_tap_ms.lock().unwrap() += feed_ms;
        }
        report.record = record_started.elapsed();
        tracing::debug!(
            record_tap_ms = *state.record_tap_ms.lock().unwrap(),
            skipped_record_frames = state.skipped_record_frames.load(Ordering::SeqCst),
            "record handoff tick"
        );
    }

    // Stream, after the draw: the whole of it is one bounded try_send.
    if let Some(tx) = stream_tx {
        let handoff_started = Instant::now();
        if let Some(surface) = shared_surface.or(own_surface) {
            report.stream_sent = crate::record::stream::hand_off_stream_surface(
                &tx,
                surface,
                frame,
                &state.skipped_stream_frames,
            );
        }
        report.stream += handoff_started.elapsed();
        *state.stream_tap_ms.lock().unwrap() += report.stream.as_secs_f64() * 1000.0;
    }
    report.total = tick_started.elapsed();
    report
}

/// The render loop: tick at every master-clock frame boundary until `on_tick`
/// returns `false` (the binary never stops it). Renders even while the clock
/// is STOPPED so the operator always has a picture; a missed deadline is only
/// meaningful — and only counted — while a show clock runs.
pub async fn run_loop(
    render: &mut RenderLoop,
    state: &EngineState,
    house_rate: u32,
    mut on_tick: impl FnMut(&TickReport) -> bool,
) {
    let frame_budget = Duration::from_secs_f64(1.0 / house_rate.max(1) as f64);
    let mut next_boundary = Instant::now();
    let mut stopped_frame: u64 = 0;
    loop {
        let now = Instant::now();
        if next_boundary > now {
            tokio::time::sleep(next_boundary - now).await;
        }
        let (frame, deadline) = match state.master_frame() {
            // RUNNING: the master clock owns the frame number and the
            // deadline is real.
            Some(f) => (f, Some(frame_budget)),
            // STOPPED: still render, but nothing is counted.
            None => {
                stopped_frame = stopped_frame.wrapping_add(1);
                (stopped_frame, None)
            }
        };
        let report = run_tick(render, state, frame, deadline).await;
        if !on_tick(&report) {
            return;
        }
        next_boundary += frame_budget;
        // If we fell far behind, resynchronize rather than spiral.
        let now = Instant::now();
        if next_boundary < now {
            next_boundary = now + frame_budget;
        }
    }
}
