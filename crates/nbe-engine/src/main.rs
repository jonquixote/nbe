//! nbe-engine binary (Prompt 03 Step 1): boot config, telemetry subscriber, and
//! the control plane channel. Per prompt constraints: clock state is in an
//! Arc<EngineState>; the clock FSM is derived by MainLoop from directives so
//! other tasks can read state.

use nbe_engine::audio_driver;
use nbe_engine::channel::{self, EngineConfig};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, RecordState, SharedEngineState, SharedOutgoing};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let ctrl =
        std::env::var("NBE_CP_URL").unwrap_or_else(|_| "ws://127.0.0.1:8462/nbe/v0.3".into());
    let token = std::env::var("NBE_RENDER_TOKEN")
        .map_err(|_| anyhow::anyhow!("NBE_RENDER_TOKEN required"))?;
    let house_rate: u32 = std::env::var("NBE_HOUSE_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let state: SharedEngineState = Arc::new(EngineState::new(house_rate));
    let outgoing: SharedOutgoing = Arc::new(Default::default());

    let cfg = EngineConfig {
        control_plane_url: ctrl,
        token,
        house_rate,
        telemetry_interval_ms: 1000,
    };

    // The audio driver runs on its own task, on the audio cadence — never in
    // the render loop (SPEC §8.9, §7.13). It owns the graph, drains the
    // intents the directive path publishes, and publishes the v0.3.3 audio
    // telemetry fields. Without it the graph is a mechanism nothing drives.
    //
    // Started before the GPU: audio does not depend on wgpu, and a slow or
    // failing device init must not silence the show.
    //
    // The sink is the null sink: device glue is the recorded deferral in
    // agents/prompts/06-audio-graph.md. Everything above the sink is real.
    audio_driver::spawn(state.clone(), house_rate);

    // The render loop runs on its own task, driven by master-clock frame
    // boundaries — not a spin. It renders even while the clock is STOPPED so
    // the operator always has a picture (the fallback slate).
    let mut render = RenderLoop::new(state.clone()).await?;
    let render_state = state.clone();
    // The render/record loop below hands frames to the dedicated record
    // thread over a bounded channel; the thread owns the hardware encoder
    // across takes, and that handle is !Send (a raw VideoToolbox session
    // pointer) — so the loop runs as a thread-local task on a LocalSet
    // instead of `tokio::spawn` (whose `Send` bound forbids it). No new
    // threads: the set is driven on this thread, and the render
    // budget/deadline path is untouched by the choice of spawner.
    let render_loops = tokio::task::LocalSet::new();
    render_loops.spawn_local(async move {
        let frame_budget = Duration::from_secs_f64(1.0 / house_rate as f64);
        let mut next_boundary = Instant::now();
        let mut stopped_frame: u64 = 0;
        // Stream live feed (WU5 FIX, additive only): Surface path, zero-copy,
        // NEVER readback/rgba here. Locals live on this LocalSet task (the
        // encoder is !Send, like the record thread's): the pool is built
        // lazily on first Live frame at View geometry (record-sized —
        // over-provisioned for a stream-only take, never under), the encoder
        // opens lazily inside `feed_stream_surface`. Costs accumulate into
        // `stream_tap_ms`, drops into `skipped_stream_frames` — never into
        // record skips nor View drops (G1). The render budget/deadline path
        // below is UNTOUCHED.
        let mut stream_pool: Option<nbe_decode::zerocopy::SurfacePool> = None;
        let mut stream_encoder: Option<nbe_decode::encode::EncodeSession> = None;
        // Per-stream live-feed state (FIX round 2, findings 1–3): the AVC
        // sequence header is per STREAM lifetime, and the directive path
        // cannot touch these loop locals (different task) — so the loop
        // observes start/stop edges itself (`note_live` below) and resets the
        // sent flag AND drops the stream-scoped encoder on every transition.
        // Without the reset a second stream reuses the first stream's sent
        // flag: it emits zero sequence headers while its fresh publisher
        // cache is empty, then caches a NALU as the replayed "seq".
        let mut stream_leg = StreamLoopState::default();
        // WU-pipe record handoff (additive only): the budget pre-check and
        // the handoff below never enter the render budget — `render_frame`
        // keeps its own deadline check UNCHANGED, and the record cost
        // (readback + handoff) accumulates into the engine-state
        // `record_tap_ms` counter. Over budget the record frame is SKIPPED
        // BEFORE the readback (record degrades, View never); on a saturated
        // handoff channel the frame is SHED (same counter). Both count in
        // `skipped_record_frames`.
        //
        // Audio note: the record thread drains the shared tap that
        // `record.start` published and the audio driver attached to the live
        // graph; the loop never touches audio.
        loop {
            let now = Instant::now();
            if next_boundary > now {
                tokio::time::sleep(next_boundary - now).await;
            }
            let (frame, deadline) = match render_state.master_frame() {
                // RUNNING: the master clock owns the frame number and the
                // deadline is real.
                Some(f) => (f, Some(frame_budget)),
                // STOPPED: still render, but a missed deadline is meaningless
                // when no show clock is running, so nothing is counted.
                None => {
                    stopped_frame = stopped_frame.wrapping_add(1);
                    (stopped_frame, None)
                }
            };
            // WU-pipe: one record frame per loop iteration, both paths.
            //
            // Budget honesty is unchanged: the budget pre-check still runs
            // AFTER the draw (it needs the View's measured time) and an
            // over-budget frame still skips with no readback, no handoff and no
            // encode. What the zero-copy path adds is an EARLIER question —
            // "is a free surface available?" — asked before the draw, because
            // on that path the draw goes into the surface and a missing one
            // costs a corrupted frame rather than a skipped one.
            //
            // One lock acquisition resolves both the take's pool and its
            // handoff endpoint, so they cannot describe different takes.
            let recording = *render_state.record_state.lock().unwrap() == RecordState::Recording;
            let (pool, endpoint) = match recording {
                true => {
                    let g = render_state.record_session.lock().unwrap();
                    match g.as_ref() {
                        Some(s) => (s.surface_pool(), Some(s.frame_sender())),
                        None => (None, None),
                    }
                }
                false => (None, None),
            };
            // What the take CLAIMS, which is what makes a missing pool a chain
            // loss rather than an ordinary CPU take.
            let claims_zero_copy = matches!(
                *render_state.record_tap_selection.lock().unwrap(),
                Some(sel) if sel.path == nbe_engine::record::tap_path::TapPath::ZeroCopy
            ) && recording;
            let loan = match nbe_engine::record::begin_tap_frame(
                &mut render,
                pool.as_deref(),
                claims_zero_copy,
                &render_state.skipped_record_frames,
            ) {
                Ok(loan) => loan,
                // Option A: the take ends here, loudly, rather than recording
                // frames through a transport nobody chose. The View is
                // unaffected and still goes on air this frame.
                Err(e) => {
                    nbe_engine::record::end_take_on_chain_loss(&render_state, &e.to_string());
                    Default::default()
                }
            };
            // Stream take (BEFORE draw, stream-only takes only): when the
            // record take already retargeted, the stream shares that loan
            // (cloned AFTER draw — one composite, N holders, G1). When only
            // the stream is live, it needs its own surface to draw into:
            // acquire here; on empty pool count ONE stream drop now and draw
            // to built-in (View draws regardless — drop-Arc, not skip-draw).
            // NEVER readback on this path.
            let streaming =
                *render_state.stream_state.lock().unwrap() == nbe_engine::state::StreamState::Live;
            // Start/stop edge: reset per-stream feed state (finding 1). The
            // encoder reopens lazily on the next Live frame.
            stream_leg.note_live(streaming, &mut stream_encoder);
            let mut stream_only_surface: Option<
                std::sync::Arc<nbe_decode::zerocopy::SharedSurface>,
            > = None;
            let mut stream_pre_dropped = false;
            if streaming && !recording {
                if stream_pool.is_none() {
                    if let Some(dev) = render_state.render_device() {
                        // Record-sized (over-provisioned for stream-only, never
                        // under): reuses the one sizing rule rather than
                        // inventing a second pool geometry.
                        if let Ok(p) = nbe_engine::record::zerocopy_pool(
                            &dev,
                            nbe_engine::render::VIEW_W,
                            nbe_engine::render::VIEW_H,
                        ) {
                            stream_pool = Some(p);
                        }
                    }
                }
                if let Some(p) = stream_pool.as_ref() {
                    match p.acquire() {
                        // Retarget failures (geometry) drop the frame —
                        // the View still draws to built-in below.
                        Some(s) if render.set_view_surface(Some(s.clone())).is_ok() => {
                            stream_only_surface = Some(s);
                        }
                        _ => {
                            render_state
                                .skipped_stream_frames
                                .fetch_add(1, Ordering::SeqCst);
                            stream_pre_dropped = true;
                        }
                    }
                } else {
                    render_state
                        .skipped_stream_frames
                        .fetch_add(1, Ordering::SeqCst);
                    stream_pre_dropped = true;
                }
            }

            let render_started = Instant::now();
            let _ = render.render_frame(frame, deadline);
            let render_elapsed = render_started.elapsed();
            // The View goes back to the built-in target the moment the draw is
            // done, before anything can fail: a retarget left in place would
            // composite the NEXT frame into a surface nobody is holding.
            // (Covers the stream-only retarget above as well — same rule.)
            nbe_engine::record::restore_view(&mut render, &loan);
            if stream_only_surface.is_some() {
                let _ = render.set_view_surface(None);
            }

            // Both-live share (G1: one composite, N Arc holders): clone the
            // record loan's surface BEFORE the record branch moves the loan.
            // The clone is read-only — the loan flows to `end_tap_frame`
            // exactly as before, so record's shed-before-draw is untouched.
            let shared_surface = loan.surface();

            if recording {
                if let Some(tx) = endpoint {
                    let feed_ms = nbe_engine::record::end_tap_frame(
                        loan,
                        render_elapsed,
                        deadline,
                        &tx,
                        &render_state.skipped_record_frames,
                        || async {
                            // Timed readback (the only await, and only on the
                            // CPU path): its cost belongs to the record
                            // counter, never the render budget.
                            let started = Instant::now();
                            let rgba = render.readback_view().await;
                            (rgba, started.elapsed())
                        },
                    )
                    .await;
                    *render_state.record_tap_ms.lock().unwrap() += feed_ms;
                }
                tracing::debug!(
                    record_tap_ms = *render_state.record_tap_ms.lock().unwrap(),
                    skipped_record_frames =
                        render_state.skipped_record_frames.load(Ordering::SeqCst),
                    "record handoff tick"
                );
            }
            // Stream live feed (Surface path, zero-copy, NEVER readback):
            // the View already drew regardless above; here the stream takes
            // the drawn surface or drops it (G1 drop-Arc). Encode runs
            // LOCK-FREE (no session lock anywhere on that path); the session
            // lock spans ONLY the bounded publish below, so `stream.stop`
            // never waits on the encoder (finding 3). Feed cost lands in
            // `stream_tap_ms`, never the render budget.
            if streaming {
                if recording {
                    match shared_surface {
                        Some(surf) => {
                            // Both-live (finding 2): share the record loan —
                            // the stream holds the Arc only across encode +
                            // bounded publish, then drops it (never blocks
                            // the draw, never becomes a record skip or a View
                            // drop; transient pressure surfaces as honest
                            // record pre-draw skips, never network-dependent).
                            feed_stream_leg(
                                &render_state,
                                &mut stream_encoder,
                                &mut stream_leg,
                                surf,
                            );
                        }
                        None => {
                            // No record surface to share (pre-draw skip, or a
                            // CPU take that drew built-in): ONE honest stream
                            // drop — record and View untouched.
                            render_state
                                .skipped_stream_frames
                                .fetch_add(1, Ordering::SeqCst);
                        }
                    }
                } else if !stream_pre_dropped {
                    if let Some(surf) = stream_only_surface {
                        feed_stream_leg(&render_state, &mut stream_encoder, &mut stream_leg, surf);
                    }
                    // (stream_pre_dropped frames already counted ONE drop at
                    // acquire; nothing more to do — the View drew regardless.)
                }
            }
            next_boundary += frame_budget;
            // If we fell far behind, resynchronize rather than spiral.
            let now = Instant::now();
            if next_boundary < now {
                next_boundary = now + frame_budget;
            }
        }
    });

    render_loops
        .run_until(channel::run_forever(cfg, state, outgoing))
        .await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Stream-leg loop state (WU5 FIX round 2, findings 1–3).
// ---------------------------------------------------------------------------

/// Per-stream live-feed state owned by the render-loop task.
///
/// `seq_sent` tracks the AVC sequence header per STREAM lifetime;
/// `was_live` observes `stream_state` edges because the directive path cannot
/// touch these loop locals (different task, no channel) — the loop resets on
/// the edge instead.
#[derive(Debug, Default)]
struct StreamLoopState {
    seq_sent: bool,
    was_live: bool,
    /// Edges observed (for tests; the loop ignores the count).
    transitions: u64,
}

impl StreamLoopState {
    /// Observe one tick's liveness. On every start/stop edge: clear the
    /// per-stream sent flag and drop the stream-scoped encoder (it reopens
    /// lazily on the next Live frame at that frame's geometry). Returns true
    /// on an edge. No edge → no touch (a live stream keeps its flag and its
    /// warm encoder across ticks).
    fn note_live(
        &mut self,
        live: bool,
        encoder: &mut Option<nbe_decode::encode::EncodeSession>,
    ) -> bool {
        if live == self.was_live {
            return false;
        }
        self.was_live = live;
        self.seq_sent = false;
        *encoder = None;
        self.transitions += 1;
        true
    }
}

/// One stream frame: encode lock-free, then publish under a brief session
/// lock.
///
/// Both the stream-only leg and the both-live leg (G1 share of the record
/// loan) call this — one composite, one helper. `encode_stream_frame` takes
/// no session and no lock, so `stream.stop` (which takes `stream_session`)
/// never waits on the encoder; the guard below spans only bounded
/// `try_send`s. Cost lands in `stream_tap_ms`, never the render budget.
fn feed_stream_leg(
    render_state: &nbe_engine::state::EngineState,
    stream_encoder: &mut Option<nbe_decode::encode::EncodeSession>,
    stream_leg: &mut StreamLoopState,
    surf: std::sync::Arc<nbe_decode::zerocopy::SharedSurface>,
) {
    let (encode_ms, payload) = nbe_engine::record::stream::encode_stream_frame(
        &surf,
        stream_encoder,
        !stream_leg.seq_sent,
        &render_state.skipped_stream_frames,
    );
    let mut feed_ms = encode_ms;
    if let Some(payload) = payload {
        let guard = render_state.stream_session.lock().unwrap();
        match guard.as_ref() {
            Some(sess) => {
                feed_ms += nbe_engine::record::stream::publish_stream_frame(
                    sess,
                    payload.seq_header,
                    &mut stream_leg.seq_sent,
                    &payload.units,
                    &render_state.skipped_stream_frames,
                );
            }
            None => {
                // Stop landed mid-tick (or a session-less Live seam): the
                // frame is honestly dropped.
                render_state
                    .skipped_stream_frames
                    .fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    *render_state.stream_tap_ms.lock().unwrap() += feed_ms;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_edges_reset_seq_and_drop_encoder() {
        let mut leg = StreamLoopState::default();
        let mut encoder: Option<nbe_decode::encode::EncodeSession> = None;
        // Idle ticks: no edge, no touch.
        assert!(!leg.note_live(false, &mut encoder));
        assert_eq!(leg.transitions, 0);
        // First start: edge (flag already clear, encoder already None).
        assert!(leg.note_live(true, &mut encoder));
        assert!(!leg.seq_sent);
        assert!(encoder.is_none());
        // Live ticks: no edge — a warm stream keeps flag and encoder.
        leg.seq_sent = true;
        assert!(!leg.note_live(true, &mut encoder));
        assert!(leg.seq_sent, "no edge must preserve the sent flag");
        assert_eq!(leg.transitions, 1);
        // Stop edge: stale flag clears even though nothing is live.
        assert!(leg.note_live(false, &mut encoder));
        assert!(!leg.seq_sent);
        // Second start (finding 1): the previous stream's sent flag clears.
        leg.seq_sent = true;
        assert!(leg.note_live(true, &mut encoder));
        assert!(!leg.seq_sent, "a second stream must emit its seq header");
        assert_eq!(leg.transitions, 3);
    }

    #[test]
    fn stream_stop_edge_drops_a_live_encoder() {
        if !nbe_engine::record::session::encoder_available() {
            eprintln!("SKIP: no hardware H.264 encoder on this machine");
            return;
        }
        let mut leg = StreamLoopState::default();
        let mut encoder: Option<nbe_decode::encode::EncodeSession> = None;
        assert!(leg.note_live(true, &mut encoder));
        encoder = Some(nbe_decode::encode::EncodeSession::open(640, 360, 30, 1_000_000).unwrap());
        assert!(leg.note_live(false, &mut encoder));
        assert!(
            encoder.is_none(),
            "stop must drop the stream-scoped encoder"
        );
    }
}
