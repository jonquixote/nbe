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

use crate::record::{CountSkip, SkipPath};
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
    /// From the tick's start to the View draw's start: the stream's surface
    /// loan and the record pool's.
    pub before_draw: Duration,
    /// How long after the next master frame's boundary the View draw was
    /// submitted, when it was: a dropped frame (SPEC §10.2). `None` on time,
    /// and while the clock is STOPPED.
    pub submitted_late: Option<Duration>,
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
    /// R21's lines have their own budget, so a burst of skips cannot hide a
    /// late submit.
    late_lines: LineBudget,
    /// How long the last tick ran (its timed region), so a drop's line (R33's
    /// skip, R21's late submit) says whether the tick before it overran or
    /// the wake came late.
    last_tick: Duration,
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
                // A master frame never drawn was never submitted by its
                // deadline: each is a dropped frame (§10.2, ruled 2026-10-28).
                let dropped_total = state
                    .dropped_frames_total
                    .fetch_add(skipped, Ordering::SeqCst)
                    + skipped;
                if let Some(suppressed) = self.lines.take(Instant::now()) {
                    tracing::warn!(
                        master_frame = f,
                        skipped,
                        previous_tick_ms = self.last_tick.as_secs_f64() * 1000.0,
                        woke_late_ms = woke_late.as_secs_f64() * 1000.0,
                        dropped_total,
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

    /// The tick just run took `total` (its timed region).
    fn ticked(&mut self, total: Duration) {
        self.last_tick = total;
    }

    /// R21: a frame drawn, but submitted after the next frame's boundary, is
    /// a drop that skips nothing, so no R33 line names it. This line does
    /// (rate-limited), with where the frame's time went: how long the tick
    /// before it ran, how late the wake came, the work before the draw, and
    /// the draw. A drop can then be tied to a late wake or to a slow draw, and
    /// a wake made late by the tick before it overrunning is not mistaken for
    /// a late timer: the loop does not sleep when that tick ended past this
    /// one's boundary, so the overrun is part of `woke_late_ms`.
    ///
    /// Called before [`Cadence::ticked`] records this tick, so
    /// `previous_tick_ms` is the tick before it, as on R33's line.
    ///
    /// Both drop lines carry `dropped_total`: `droppedFramesTotal` right after
    /// the drops the line names were counted. Only the render thread moves
    /// that counter, and nothing moves it between `run_tick`'s count and this
    /// line, so a reader of the telemetry knows exactly which counts a line
    /// explains (the dress's R21 backstop).
    fn submitted_late(&mut self, state: &EngineState, report: &TickReport) {
        let (Some(f), Some(late)) = (report.master_frame, report.submitted_late) else {
            return;
        };
        if let Some(suppressed) = self.late_lines.take(Instant::now()) {
            tracing::warn!(
                master_frame = f,
                late_ms = late.as_secs_f64() * 1000.0,
                previous_tick_ms = self.last_tick.as_secs_f64() * 1000.0,
                woke_late_ms = report.woke_late.as_secs_f64() * 1000.0,
                before_draw_ms = report.before_draw.as_secs_f64() * 1000.0,
                render_ms = report.render.as_secs_f64() * 1000.0,
                dropped_total = state.dropped_frames_total.load(Ordering::SeqCst),
                suppressed,
                "R21: master frame {f} was drawn but submitted after frame {}'s boundary",
                f + 1
            );
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

    /// R33's run edge: master frames `first..=last` the clock passed at a
    /// run's start before the loop's first tick on it, or at its stop after
    /// the loop's last (#77's precision note 1). Each is a master frame never
    /// drawn, so a dropped frame (§10.2, ruled 2026-10-28), counted with the
    /// skips. A stop's edge lands in the run it ends, a start's in the run
    /// it begins.
    fn edge(&mut self, state: &EngineState, first: u64, last: u64, at: &str, woke_late: Duration) {
        let skipped = last - first + 1;
        state
            .skipped_master_frames
            .fetch_add(skipped, Ordering::SeqCst);
        state
            .dropped_frames_total
            .fetch_add(skipped, Ordering::SeqCst);
        if let Some(suppressed) = self.lines.take(Instant::now()) {
            tracing::warn!(
                skipped,
                previous_tick_ms = self.last_tick.as_secs_f64() * 1000.0,
                woke_late_ms = woke_late.as_secs_f64() * 1000.0,
                suppressed,
                "R33: master frames {first}..={last} were never drawn {at}"
            );
        }
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
        &*state.skipped_record_frames,
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
                state.skipped_stream_frames.count(SkipPath::NoSurface, 1);
            }
        }
    }
    report.stream += stream_started.elapsed();

    let render_started = Instant::now();
    report.before_draw = render_started - tick_started;
    let _ = render.render_frame(frame, deadline);
    report.render = render_started.elapsed();
    // SPEC §10.2, as the user ruled on 2026-10-28: a View frame is dropped
    // when the master clock reaches the next frame's boundary before it was
    // submitted. Counted only while the clock runs, once per master frame: a
    // frame never drawn is counted by the loop's cadence instead. The loop's
    // cadence logs this drop too (R21), since no frame was skipped.
    if deadline.is_some() {
        let submitted = Instant::now();
        if let Some(next) = state.frame_boundary(frame + 1).filter(|&n| submitted >= n) {
            state.dropped_frames_total.fetch_add(1, Ordering::SeqCst);
            report.submitted_late = Some(submitted - next);
        }
    }
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
                &*state.skipped_record_frames,
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
                &*state.skipped_stream_frames,
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
    // Whether the loop has ticked yet: the frames a clock passed before the
    // loop existed are not the loop's to count.
    let mut ticked = false;
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
        // R33's run edges. The run drawn last ended since the last tick
        // (stopped, or restarted on a new epoch): the frames its clock passed
        // after the last one drawn were never drawn. The frame the clock
        // stopped in is not among them: the clock never reached its
        // successor's boundary. And a run this tick is the first on: the
        // frames before the one it reads were never drawn. Both are normally
        // none; a stall across the edge is what they count.
        let now_epoch = reading.map(|(epoch, _)| epoch);
        let stop_edge = drawn
            .filter(|(epoch, _)| now_epoch != Some(*epoch))
            .and_then(|(epoch, last)| {
                let (stopped, at_stop) = state.last_stopped_run()?;
                (stopped == epoch && at_stop > last + 1).then_some((last + 1, at_stop - 1))
            });
        let start_edge = match reading {
            Some((epoch, first)) if ticked && first > 0 && drawn.map(|(e, _)| e) != Some(epoch) => {
                Some((0, first - 1))
            }
            _ => None,
        };
        if let Some((first, last)) = stop_edge {
            cadence.edge(state, first, last, "before the clock stopped", woke_late);
        }
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
        if let Some((first, last)) = start_edge {
            cadence.edge(state, first, last, "at the run's start", woke_late);
        }
        ticked = true;
        let mut report = run_tick(render, state, frame, deadline).await;
        report.master_frame = master;
        report.woke_late = woke_late;
        cadence.submitted_late(state, &report);
        cadence.ticked(report.total);
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
        // Each master frame never drawn is a dropped frame (§10.2, ruled
        // 2026-10-28); a repeat is not.
        assert_eq!(state.dropped_frames_total.load(Ordering::SeqCst), 1 + 3);
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
        // One drawn frame late by the clock, beside the 2 never drawn.
        state.dropped_frames_total.fetch_add(1, Ordering::SeqCst);
        let take = cadence.take.expect("a recording tick opens the take");
        assert_eq!(
            take.summary("record take", &state),
            "cadence: record take ended after 3 ticks: repeats 1, skips 2, drops 3; \
             woke late mean 13660 µs, max 40.00 ms; ≤0.1 ms 1, ≤0.25 0, ≤0.5 0, ≤1 1, ≤2 0, ≤5 0, ≤10 0, ≤1 frame 0, >1 frame 1"
        );
    }

    thread_local! {
        /// This thread's capture, while [`captured`] runs on it.
        static CAPTURE: std::cell::RefCell<Option<Vec<u8>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// The lib tests' one subscriber's writer: each event goes to the buffer
    /// of the thread that emitted it while [`captured`] runs there, and is
    /// dropped otherwise.
    struct ThreadBuffer;

    impl std::io::Write for ThreadBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            CAPTURE.with(|capture| {
                if let Some(log) = capture.borrow_mut().as_mut() {
                    log.extend_from_slice(buf);
                }
            });
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// What `f` logged on this thread, as text.
    ///
    /// One global subscriber, installed once, writes each thread's events to
    /// that thread's own buffer (the user's rulings of 2026-11-05, item 2). A
    /// scoped subscriber per capture (`with_default`) missed events while the
    /// lib's other tests ran in parallel with none:
    /// `a_burst_of_skips_cannot_hide_a_late_submit` lost every R33 line in 6
    /// to 9 of 40 parallel runs, and none alone or on one thread. With one
    /// global subscriber no scoped dispatcher comes and goes during a run.
    /// The interest cache is rebuilt once after the install, so a callsite
    /// registered before it sees the subscriber.
    fn captured(f: impl FnOnce()) -> String {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(|| ThreadBuffer)
                .with_ansi(false)
                .finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("the lib tests' only subscriber");
            tracing::callsite::rebuild_interest_cache();
        });
        CAPTURE.with(|capture| *capture.borrow_mut() = Some(Vec::new()));
        f();
        let bytes = CAPTURE.with(|capture| capture.borrow_mut().take());
        String::from_utf8(bytes.unwrap_or_default()).unwrap()
    }

    #[test]
    fn a_burst_of_skips_cannot_hide_a_late_submit() {
        let text = captured(|| {
            let state = EngineState::new(30);
            let mut cadence = Cadence::default();
            // Every other frame skipped: more R33 lines than a second allows.
            let frames: Vec<_> = (0..=2 * R33_LINES_PER_SECOND as u64 + 2)
                .map(|i| Some(2 * i))
                .collect();
            feed(&mut cadence, &state, &frames);
            cadence.submitted_late(
                &state,
                &TickReport {
                    master_frame: Some(99),
                    submitted_late: Some(Duration::from_millis(3)),
                    ..Default::default()
                },
            );
        });
        assert!(
            text.matches("R33: ").count() >= R33_LINES_PER_SECOND as usize,
            "the skips used up R33's budget:\n{text}"
        );
        assert!(
            text.contains(
                "R21: master frame 99 was drawn but submitted after frame 100's boundary"
            ),
            "a late submit after R33's budget is spent still has its line:\n{text}"
        );
    }

    /// The R21 backstop's join: each drop line names the `droppedFramesTotal`
    /// its own count reached, so the dress knows which counts it explains.
    #[test]
    fn each_drop_line_carries_the_dropped_total_its_own_count_reached() {
        let text = captured(|| {
            let state = EngineState::new(30);
            state.dropped_frames_total.store(5, Ordering::SeqCst);
            let mut cadence = Cadence::default();
            feed(&mut cadence, &state, &[Some(10), Some(13)]);
            // `run_tick`'s count of frame 13's late submit, then its line.
            state.dropped_frames_total.fetch_add(1, Ordering::SeqCst);
            cadence.submitted_late(
                &state,
                &TickReport {
                    master_frame: Some(13),
                    submitted_late: Some(Duration::from_millis(3)),
                    ..Default::default()
                },
            );
            feed(&mut cadence, &state, &[Some(15)]);
        });
        let line = |what: &str| {
            text.lines()
                .find(|l| l.contains(what))
                .unwrap_or_else(|| panic!("no line `{what}` in:\n{text}"))
                .to_string()
        };
        assert!(line("R33: master frames 11..=12 were never drawn").contains("dropped_total=7"));
        assert!(line("R21: master frame 13 was drawn").contains("dropped_total=8"));
        assert!(line("R33: master frames 14..=14 were never drawn").contains("dropped_total=9"));
    }

    /// R21's line says how long the tick before it ran, as R33's does: the
    /// tick before overrunning is a late wake no timer caused.
    #[test]
    fn the_late_submit_line_names_the_tick_before_it() {
        let text = captured(|| {
            let state = EngineState::new(30);
            let mut cadence = Cadence::default();
            feed(&mut cadence, &state, &[Some(20)]);
            cadence.ticked(Duration::from_millis(45));
            feed(&mut cadence, &state, &[Some(21)]);
            cadence.submitted_late(
                &state,
                &TickReport {
                    master_frame: Some(21),
                    submitted_late: Some(Duration::from_millis(3)),
                    total: Duration::from_millis(7),
                    ..Default::default()
                },
            );
        });
        let line = text
            .lines()
            .find(|l| l.contains("R21: master frame 21 was drawn"))
            .unwrap_or_else(|| panic!("no R21 line in:\n{text}"));
        assert!(
            line.contains("previous_tick_ms=45"),
            "the line names the tick before it (45 ms), not its own: {line}"
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
