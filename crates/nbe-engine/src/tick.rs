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
    /// The master frame this tick drew; `None` while the clock is STOPPED
    /// (the loop's own frame number then, which nothing counts).
    pub master_frame: Option<u64>,
    /// How late the loop woke against the instant it slept for: the timer's
    /// granularity and the scheduler's, which differ by OS (R33).
    pub woke_late: Duration,
}

/// Wake-lateness buckets' upper edges, in microseconds; past the last edge a
/// wake falls in "within one frame" or "over one frame".
const WAKE_EDGES_US: [u64; 7] = [100, 250, 500, 1_000, 2_000, 5_000, 10_000];

/// How late the loop woke, over a span of ticks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WakeStats {
    pub ticks: u64,
    pub total: Duration,
    pub max: Duration,
    /// Wakes at or under each of [`WAKE_EDGES_US`], then within one frame,
    /// then over one frame.
    pub buckets: [u64; 9],
}

impl WakeStats {
    fn add(&mut self, late: Duration, budget: Duration) {
        self.ticks += 1;
        self.total += late;
        self.max = self.max.max(late);
        let us = late.as_micros() as u64;
        let i = match WAKE_EDGES_US.iter().position(|&edge| us <= edge) {
            Some(i) => i,
            None if late <= budget => 7,
            None => 8,
        };
        self.buckets[i] += 1;
    }

    fn line(&self) -> String {
        let mean_us = match self.ticks {
            0 => 0,
            n => self.total.as_micros() as u64 / n,
        };
        let b = &self.buckets;
        format!(
            "woke late mean {mean_us} µs, max {:.2} ms; ≤0.1 ms {}, ≤0.25 {}, ≤0.5 {}, ≤1 {}, ≤2 {}, ≤5 {}, ≤10 {}, ≤1 frame {}, >1 frame {}",
            self.max.as_secs_f64() * 1000.0,
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8]
        )
    }
}

/// R33 lines allowed per second; the rest are counted and reported with the
/// next line let through.
const R33_LINES_PER_SECOND: u32 = 5;

#[derive(Default)]
struct LineBudget {
    window: Option<Instant>,
    used: u32,
    suppressed: u64,
}

impl LineBudget {
    /// `Some(lines suppressed since the last one let through)` when a line
    /// may go now.
    fn take(&mut self, now: Instant) -> Option<u64> {
        match self.window {
            Some(w) if now.duration_since(w) < Duration::from_secs(1) => {}
            _ => {
                self.window = Some(now);
                self.used = 0;
            }
        }
        if self.used < R33_LINES_PER_SECOND {
            self.used += 1;
            Some(std::mem::take(&mut self.suppressed))
        } else {
            self.suppressed += 1;
            None
        }
    }
}

/// The counters at a span's start, and the span's own ticks and wakes.
#[derive(Clone, Copy)]
struct Span {
    ticks: u64,
    repeated: u64,
    skipped: u64,
    dropped: u64,
    wake: WakeStats,
}

impl Span {
    fn open(state: &EngineState) -> Self {
        Self {
            ticks: 0,
            repeated: state.repeated_master_frames.load(Ordering::SeqCst),
            skipped: state.skipped_master_frames.load(Ordering::SeqCst),
            dropped: state.dropped_frames_total.load(Ordering::SeqCst),
            wake: WakeStats::default(),
        }
    }

    fn summary(&self, what: &str, state: &EngineState) -> String {
        format!(
            "cadence: {what} ended after {} ticks: repeats {}, skips {}, drops {}; {}",
            self.ticks,
            state.repeated_master_frames.load(Ordering::SeqCst) - self.repeated,
            state.skipped_master_frames.load(Ordering::SeqCst) - self.skipped,
            state.dropped_frames_total.load(Ordering::SeqCst) - self.dropped,
            self.wake.line()
        )
    }
}

/// R33's accounting, kept by the loop: a running tick that draws the master
/// frame the tick before it drew is a repeat, and a master frame no running
/// tick drew is a skip. Each is logged with its master frame (rate-limited),
/// and a run (the clock RUNNING) or a record take ends with a one-line
/// summary of its repeats, skips, drops and wakes. Internal and logged; the
/// counters never reach the wire (the user's ruling of 2026-10-28).
#[derive(Default)]
struct Cadence {
    /// The master frame the last running tick drew.
    last: Option<u64>,
    run: Option<Span>,
    take: Option<Span>,
    lines: LineBudget,
}

impl Cadence {
    fn tick(
        &mut self,
        state: &EngineState,
        master: Option<u64>,
        recording: bool,
        woke_late: Duration,
        budget: Duration,
    ) {
        // A span closes before this tick is counted and opens at it.
        let restarted = matches!((master, self.last), (Some(f), Some(prev)) if f < prev);
        if master.is_none() || restarted {
            if let Some(run) = self.run.take() {
                tracing::info!("{}", run.summary("run", state));
            }
            self.last = None;
        }
        if master.is_some() && self.run.is_none() {
            self.run = Some(Span::open(state));
        }
        match (recording, self.take.is_some()) {
            (false, true) => {
                if let Some(take) = self.take.take() {
                    tracing::info!("{}", take.summary("record take", state));
                }
            }
            (true, false) => self.take = Some(Span::open(state)),
            _ => {}
        }

        if let (Some(f), Some(prev)) = (master, self.last) {
            if f == prev {
                state.repeated_master_frames.fetch_add(1, Ordering::SeqCst);
                if let Some(suppressed) = self.lines.take(Instant::now()) {
                    tracing::warn!(
                        master_frame = f,
                        suppressed,
                        "R33: the tick drew master frame {f} again"
                    );
                }
            } else if f > prev + 1 {
                let skipped = f - prev - 1;
                state
                    .skipped_master_frames
                    .fetch_add(skipped, Ordering::SeqCst);
                if let Some(suppressed) = self.lines.take(Instant::now()) {
                    tracing::warn!(
                        master_frame = f,
                        skipped,
                        suppressed,
                        "R33: master frames {}..={} were never drawn",
                        prev + 1,
                        f - 1
                    );
                }
            }
        }
        if master.is_some() {
            self.last = master;
        }
        for span in [&mut self.run, &mut self.take].into_iter().flatten() {
            span.ticks += 1;
            span.wake.add(woke_late, budget);
        }
    }

    /// The clock restarted between two ticks (a new epoch): the run so far
    /// ends, and the next frame starts the new run's count.
    fn new_run(&mut self, state: &EngineState) {
        if let Some(run) = self.run.take() {
            tracing::info!("{}", run.summary("run", state));
        }
        self.last = None;
    }

    /// The loop is returning: close whatever span is open.
    fn end(&mut self, state: &EngineState) {
        if let Some(take) = self.take.take() {
            tracing::info!("{}", take.summary("record take", state));
        }
        if let Some(run) = self.run.take() {
            tracing::info!("{}", run.summary("run", state));
        }
    }
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
    let mut loan = match crate::record::begin_tap_frame(
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
    // R31: which submission drew this frame's View, for the record thread to
    // wait on before the encoder reads the surface (the render thread never
    // waits on it).
    loan.mark_drawn(render, frame);
    // R35: the same draw, for the stream thread to wait on before its encoder
    // reads whichever surface the stream takes (the record loan's or its own).
    let stream_drawn = match stream_tx {
        Some(_) => crate::record::DrawDone::of_view(render),
        None => None,
    };
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
                stream_drawn,
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
///
/// **R33: the clock names the frame, once, in order.** While the clock runs,
/// the loop sleeps to the clock's own boundary for the frame after the last
/// one it drew, then draws the frame the clock names. A timer wakes late, by
/// an amount that differs by OS, and a late wake still names the frame it
/// slept for; a wake more than a frame late names a later one, and the frames
/// passed over are skips, counted. A frame already drawn is never drawn
/// again, and frames passed over are never drawn to catch up. STOPPED keeps
/// the loop's own grid. The loop used to sleep to its own grid and read the
/// clock wherever it woke, so a grid near a clock boundary drew one frame
/// twice and never the next (630, 630, 632).
pub async fn run_loop(
    render: &mut RenderLoop,
    state: &EngineState,
    house_rate: u32,
    mut on_tick: impl FnMut(&TickReport) -> bool,
) {
    let frame_budget = Duration::from_secs_f64(1.0 / house_rate.max(1) as f64);
    let mut next_boundary = Instant::now();
    let mut stopped_frame: u64 = 0;
    let mut cadence = Cadence::default();
    // The running clock's epoch and the master frame last drawn on it.
    let mut drawn: Option<(Instant, u64)> = None;
    loop {
        let intended = match (state.clock_reading(), drawn) {
            (Some((epoch, _)), Some((drawn_epoch, last))) if epoch == drawn_epoch => {
                state.frame_boundary(last + 1).unwrap_or(next_boundary)
            }
            // The clock's first running tick, or a new run: draw now.
            (Some(_), _) => Instant::now(),
            (None, _) => next_boundary,
        };
        let now = Instant::now();
        if intended > now {
            tokio::time::sleep(intended - now).await;
        }
        let woke_late = Instant::now().saturating_duration_since(intended);
        let reading = state.clock_reading();
        // A wake at or after `boundary(last + 1)` reads `last + 1` or later:
        // the clock's arithmetic is exact (clock.rs pins the agreement), and a
        // sleep never ends before its deadline.
        if let (Some((epoch, _)), Some((drawn_epoch, _))) = (reading, drawn) {
            if epoch != drawn_epoch {
                cadence.new_run(state);
            }
        }
        drawn = reading;
        let master = reading.map(|(_, f)| f);
        let (frame, deadline) = match master {
            // RUNNING: the master clock owns the frame number and the
            // deadline is real.
            Some(f) => (f, Some(frame_budget)),
            // STOPPED: still render, but nothing is counted.
            None => {
                stopped_frame = stopped_frame.wrapping_add(1);
                (stopped_frame, None)
            }
        };
        let recording = *state.record_state.lock().unwrap() == RecordState::Recording;
        cadence.tick(state, master, recording, woke_late, frame_budget);
        let mut report = run_tick(render, state, frame, deadline).await;
        report.master_frame = master;
        report.woke_late = woke_late;
        if !on_tick(&report) {
            cadence.end(state);
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

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: Duration = Duration::from_nanos(33_333_333);

    fn feed(cadence: &mut Cadence, state: &EngineState, frames: &[Option<u64>]) {
        for &f in frames {
            cadence.tick(state, f, false, Duration::ZERO, BUDGET);
        }
    }

    #[test]
    fn a_repeated_master_frame_is_counted_once_per_repeat() {
        let state = EngineState::new(30);
        let mut cadence = Cadence::default();
        feed(
            &mut cadence,
            &state,
            &[
                Some(629),
                Some(630),
                Some(630),
                Some(631),
                Some(631),
                Some(631),
            ],
        );
        assert_eq!(state.repeated_master_frames.load(Ordering::SeqCst), 3);
        assert_eq!(state.skipped_master_frames.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_skipped_master_frame_counts_every_frame_never_drawn() {
        // R33's own example: 630, 630, then 632 never draws 631.
        let state = EngineState::new(30);
        let mut cadence = Cadence::default();
        feed(
            &mut cadence,
            &state,
            &[Some(630), Some(630), Some(632), Some(636)],
        );
        assert_eq!(state.repeated_master_frames.load(Ordering::SeqCst), 1);
        assert_eq!(state.skipped_master_frames.load(Ordering::SeqCst), 1 + 3);
    }

    #[test]
    fn a_stopped_clock_or_a_restart_is_neither_a_repeat_nor_a_skip() {
        let state = EngineState::new(30);
        let mut cadence = Cadence::default();
        // Running, stopped (nothing counted), running again from a new epoch,
        // then a restart between two ticks (the frame goes backwards).
        feed(
            &mut cadence,
            &state,
            &[
                Some(10),
                Some(11),
                None,
                None,
                Some(0),
                Some(1),
                Some(0),
                Some(1),
            ],
        );
        assert_eq!(state.repeated_master_frames.load(Ordering::SeqCst), 0);
        assert_eq!(state.skipped_master_frames.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_span_summarises_its_own_repeats_skips_drops_and_wakes() {
        let state = EngineState::new(30);
        state.repeated_master_frames.store(7, Ordering::SeqCst);
        state.dropped_frames_total.store(2, Ordering::SeqCst);
        let mut cadence = Cadence::default();
        cadence.tick(&state, Some(100), true, Duration::from_micros(80), BUDGET);
        cadence.tick(&state, Some(100), true, Duration::from_micros(900), BUDGET);
        cadence.tick(&state, Some(103), true, Duration::from_millis(40), BUDGET);
        state.dropped_frames_total.fetch_add(1, Ordering::SeqCst);
        let take = cadence.take.expect("a recording tick opens the take");
        assert_eq!(
            take.summary("record take", &state),
            "cadence: record take ended after 3 ticks: repeats 1, skips 2, drops 1; \
             woke late mean 13660 µs, max 40.00 ms; ≤0.1 ms 1, ≤0.25 0, ≤0.5 0, ≤1 1, ≤2 0, ≤5 0, ≤10 0, ≤1 frame 0, >1 frame 1"
        );
    }

    #[test]
    fn r33_lines_are_rate_limited_and_the_suppressed_are_reported() {
        let mut lines = LineBudget::default();
        let t0 = Instant::now();
        let let_through: Vec<_> = (0..8).map(|_| lines.take(t0)).collect();
        assert_eq!(
            let_through.iter().filter(|l| l.is_some()).count(),
            R33_LINES_PER_SECOND as usize
        );
        assert_eq!(lines.take(t0 + Duration::from_secs(1)), Some(3));
    }
}
