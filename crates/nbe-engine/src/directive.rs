//! Directive intake (Prompt 03 Step 3/3a/4a): apply control-plane directives
//! to the engine's clock/state, verify fallback residency at show.load, and
//! acknowledge independently of the command path.

use crate::render::{VIEW_H, VIEW_W};
use crate::state::{FallbackSlate, RecordState, SharedEngineState, SharedOutgoing, StreamState};
use nbe_protocol::{DirectiveFrame, EngineFrame, ItemEvent};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, info};

/// GOP length assumed when the manifest does not declare one; feeds the
/// §12.8 read-ahead minimum.
const DEFAULT_GOP_FRAMES: u32 = 30;

#[derive(Debug, Error)]
pub enum DirectiveError {
    #[error("no package loaded: {0}")]
    NoPackage(String),
    #[error("fallback slate not resident: {0}")]
    FallbackMissing(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid directive: {0}")]
    Invalid(String),
    #[error("E_FORBIDDEN_STATE: {0}")]
    ForbiddenState(String),
    #[error("E_NO_HARDWARE_ENCODER: {0}")]
    NoHardwareEncoder(String),
    /// The record thread did not finish within the bounded wait
    /// (`E_RECORD_TIMEOUT`, engine-local — the spec's stop failure modes
    /// predate the threaded take). File kept as-is, take force-abandoned.
    #[error("E_RECORD_TIMEOUT: {0}")]
    Timeout(String),
    /// The requested geometry exceeds what the record path can sustain
    /// (`E_UNSUPPORTED`, engine-local — the spec has no resolution ceiling for
    /// recording). See [`check_record_resolution`].
    #[error("E_UNSUPPORTED: {0}")]
    Unsupported(String),
    #[error("E_DISK: {0}")]
    Disk(String),
}

/// Tracks the currently playing timed item so a superseding take cancels its
/// end event. Uses a generation counter: any new take bumps the generation,
/// and the spawned end-task checks "am I still the current playback?" before
/// emitting. Without this the `itemEvent: end` fires for the superseded take
/// even after a new take replaced it — a documented guarantee that must exist.
pub struct PlaybackTracker {
    current: Mutex<Option<(String, u64)>>,
}

impl PlaybackTracker {
    fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }
    /// Publish a new playing item; returns the generation for the timer.
    fn begin(&self, item_ref: &str) -> u64 {
        let mut cur = self.current.lock().unwrap();
        let generation = cur.take().map(|(_, g)| g + 1).unwrap_or(0);
        *cur = Some((item_ref.to_string(), generation));
        generation
    }
    /// True if (item_ref, generation) is still the recorded playback.
    fn is_current(&self, item_ref: &str, generation: u64) -> bool {
        matches!(&*self.current.lock().unwrap(), Some((item, g)) if item == item_ref && *g == generation)
    }
}

#[derive(Clone)]
pub struct DirectiveHandler {
    state: SharedEngineState,
    outgoing: SharedOutgoing,
    playing: Arc<PlaybackTracker>,
}

impl DirectiveHandler {
    pub fn new(state: SharedEngineState, outgoing: SharedOutgoing) -> Self {
        Self {
            state,
            outgoing,
            playing: Arc::new(PlaybackTracker::new()),
        }
    }

    /// Apply a directive; advance the last-applied stateVersion on success.
    pub async fn apply(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        match d.command.as_str() {
            // `show.load` decodes every video asset in the package — seconds of
            // blocking CPU work. Run inline on the async directive path it owned
            // the runtime: a measured 4.02 s load let no other task run at all,
            // so `show.start` could not be applied and the telemetry pump could
            // not tick. That is the whole of "R5: the clock does not start
            // promptly" — the clock was never the defect. `MasterClock` is
            // `(now - epoch) * rate` and runs from the instant `start()` is
            // called; it read 0 because `show.start` had not been applied yet,
            // and it jumped to 150 because five seconds of it had gone
            // unobserved. Blocking work belongs on the blocking pool.
            "show.load" => {
                let handler = self.clone();
                let frame = d.clone();
                tokio::task::spawn_blocking(move || handler.on_show_load(&frame))
                    .await
                    .map_err(|e| DirectiveError::Invalid(format!("show.load panicked: {e}")))??;
            }
            "show.start" => self.on_show_start(d)?,
            "show.stop" => self.on_show_stop(d).await?,
            "view.take" | "view.cut" => self.on_take(d)?,
            "view.fallback" => self.on_fallback(d)?,
            "overlay.show" | "overlay.hide" => self.on_overlay(d)?,
            "soundboard.play" | "soundboard.stop" | "soundboard.stopAll" | "audio.bus.set"
            | "audio.duck" | "guest.mute" => self.on_audio(d)?,
            "record.start" => self.on_record_start(d)?,
            "record.stop" => self.on_record_stop(d)?,
            "stream.start" => self.on_stream_start(d)?,
            "stream.stop" => self.on_stream_stop(d).await?,
            "marker.add" => self.on_marker_add(d)?,
            nbe_protocol::command::RESYNC => self.on_resync(d)?,
            other => {
                debug!(command = other, "directive ignored (no engine effect)");
            }
        }
        // Ack every applied directive (SPEC 5.9.3: appliedStateVersion is the
        // engine's most recent applied stateVersion — the honest signal for
        // /status and the show.stop grace window). One emission point, called
        // exactly once per applied directive. WU8 ordering note: record.stop
        // finalizes its file INSIDE its handler above, so this ack is only
        // observable after the file + sidecar are complete.
        self.state.set_last_applied(d.state_version);
        self.ack(d.state_version);
        Ok(())
    }

    fn on_show_load(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        // [RI-8] release-then-rebuild, FIRST: a load→load with no stop in
        // between must not stack the previous show's sessions onto the cap.
        // Outstanding leases go stale (epoch bump) and their Drop is a no-op.
        self.state.sessions.release_all();
        // No cross-show pollution: a newly loaded show never inherits the
        // previous show's markers.
        crate::record::markers::clear();
        let path = d
            .payload
            .get("packagePath")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DirectiveError::Invalid("show.load missing packagePath".into()))?;
        *self.state.package_path.lock().unwrap() = Some(path.to_string());
        let fallback = load_fallback_asset(path)?;
        *self.state.fallback.lock().unwrap() = Some(fallback);

        // Prompt 04: index the package and decode its image assets here, at
        // load time — never in the render loop (SPEC §7.13). Validity was
        // already decided by the control plane's preflight.
        let root = std::path::Path::new(path);
        let manifest: serde_json::Value = serde_json::from_reader(
            std::fs::File::open(root.join("manifest.json"))
                .map_err(|e| DirectiveError::Invalid(format!("cannot open manifest: {e}")))?,
        )
        .map_err(|e| DirectiveError::Invalid(format!("manifest not valid JSON: {e}")))?;
        let index = crate::scene::PackageIndex::build(&manifest, root);

        // Record what the show asked for; the cap is applied at publish time
        // (SPEC §10.1.1), so the order of probe and load does not matter.
        self.state.set_requested_quality(index.requested_quality);

        // The telemetry pump measures `recordSpaceMib` against this target.
        // `show.outputs.record.directory`, resolved against the package root
        // when relative; `None` when the package declares no record target.
        let record_dir = manifest
            .get("show")
            .and_then(|s| s.get("outputs"))
            .and_then(|o| o.get("record"))
            .and_then(|r| r.get("directory"))
            .and_then(|v| v.as_str())
            .map(|d| {
                let p = std::path::Path::new(d);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    root.join(p)
                }
            });
        *self.state.record_dir.lock().unwrap() = record_dir;

        // Prompt 05: decode the package's video assets here, at load time.
        // A genuine decode failure IS a fault — unlike Prompt 04's scope
        // boundary — and is reported as `itemEvent: decodeError` so the
        // control plane can drive the item to ERROR (SPEC §5.9.3, §17.3).
        let mut library = crate::video::VideoLibrary::default();
        for (asset_id, kind) in &index.asset_kind {
            if kind != "video" && kind != "alphaVideo" {
                continue;
            }
            let Some(src) = index.asset_source.get(asset_id) else {
                continue;
            };
            let declared = index.declared_loop_period.get(asset_id).copied();
            // SPEC §12.4's table, with this asset's declared ceiling where the
            // manifest declares one — the same constructor preflight uses.
            let budget = index.loop_budget(asset_id);
            match crate::video::load_video_asset(
                asset_id,
                &root.join(src),
                &self.state.sessions,
                budget,
                declared,
                DEFAULT_GOP_FRAMES,
            ) {
                Ok(asset) => {
                    info!(
                        asset = %asset_id,
                        frames = asset.frames.len(),
                        rate = asset.source_frame_rate,
                        "video asset decoded"
                    );
                    library.assets.insert(asset_id.clone(), asset);
                }
                Err(e) => {
                    // The full reason — path and platform error — stays in the
                    // engine's log. The frame that crosses the render channel
                    // carries a stable token instead, the same discipline the
                    // control plane uses for auth failures (SPEC §5.3).
                    tracing::error!(asset = %asset_id, err = %e, "video asset failed to decode");
                    library.failures.insert(asset_id.clone(), e.to_string());
                    self.state
                        .decode_failures_total
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    // `itemEvent` is addressed to a rundown Item, because that
                    // is what the §17.3 state machine tracks. Reporting an
                    // asset id here would produce a fault the control plane
                    // cannot attribute to anything.
                    let affected = index.items_using_asset(asset_id);
                    if affected.is_empty() {
                        // F1/F2. This branch used to be a `warn!` and nothing
                        // else: deleting the line left the suite green, because
                        // a log line is not an effect. The count is the effect —
                        // it is the only evidence an unattributable decode
                        // failure happened, since no Item will ever go ERROR
                        // for it.
                        self.state
                            .unattributable_decode_failures_total
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        tracing::warn!(
                            asset = %asset_id,
                            "decode failure affects no rundown item; counted, not reported as an itemEvent"
                        );
                    }
                    for item_ref in affected {
                        self.outgoing.push(EngineFrame::ItemEvent {
                            v: nbe_protocol::PROTOCOL_VERSION.to_string(),
                            item_ref,
                            event: ItemEvent::DecodeError,
                            detail: Some(e.kind_token().to_string()),
                        });
                    }
                }
            }
        }
        *self.state.video.lock().unwrap() = library;

        // SPEC §8.4: soundboard and clip audio are RAM-resident from
        // `show.load`. A trigger that reads disk cannot meet AC-13's 20 ms,
        // and a take that decodes cannot meet §7.13.
        let mut audio_assets = std::collections::BTreeMap::new();
        for (asset_id, src) in &index.asset_source {
            match nbe_decode::decode_audio(&root.join(src)) {
                Ok(Some(track)) => {
                    info!(
                        asset = %asset_id,
                        frames = track.frames(),
                        "audio asset resident"
                    );
                    audio_assets.insert(asset_id.clone(), Arc::new(track.samples));
                }
                // No audio track is normal media, not a fault.
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(asset = %asset_id, err = %e, "audio decode failed");
                }
            }
        }
        *self.state.audio_assets.lock().unwrap() = audio_assets;

        // The item→asset walk happens HERE, once, not on the take path.
        // Residency is keyed by asset id and a take carries an item ref; every
        // package used in testing named them the same string, so the mismatch
        // was invisible until the dress show (item `A1`, asset `A1_clip`) made
        // every bus read -120 dBFS with the whole audio stack working.
        // SPEC §7.2: the package declares the house rate the show was authored
        // at. The engine takes its rate from NBE_HOUSE_RATE and nothing
        // reconciled the two, so a 25 fps package loaded on a 30 fps engine
        // mapped every non-house-rate asset against the wrong denominator with
        // no path detecting it. Loud, because the picture is wrong in a way an
        // operator will see and not be able to explain.
        if let Some(declared) = index.declared_house_rate {
            let running = self.state.house_rate();
            if declared != running {
                tracing::error!(
                    declared,
                    running,
                    "package house rate does not match the engine's; timed items \
                     and non-house-rate assets will play at the wrong speed"
                );
            }
        }

        let item_audio = index.item_audio_map();
        info!(items = item_audio.len(), "item audio resolved");
        *self.state.item_audio.lock().unwrap() = item_audio;

        *self.state.package.lock().unwrap() = Some(index);
        self.state.package_generation.fetch_add(1, Ordering::SeqCst);
        *self.state.view_item.lock().unwrap() = None;
        *self.state.preview_item.lock().unwrap() = None;
        *self.state.transition.lock().unwrap() = None;
        info!(path, "show.load: package indexed, fallback resident");
        Ok(())
    }

    fn on_show_start(&self, _d: &DirectiveFrame) -> Result<(), DirectiveError> {
        self.state.clock.lock().unwrap().start();
        Ok(())
    }

    /// show.stop arrives with the quiesce stop directives already emitted by
    /// the control plane (SPEC §5.9.5). The engine applies them; the ack is
    /// emitted by `apply` on the way out, once output stopping is real.
    ///
    /// WU-pipe quiescence (§16.1 table — `quiesceOutputs` defaults true,
    /// `force` defaults false), per live output (record take and/or stream
    /// session — WU4 mirrors the record arms for the stream):
    /// * `(true, false)` with a live output: graceful internal `record.stop` /
    ///   `stream.stop` (record's bounded 1.5 s finish on the blocking pool
    ///   beside the stream's bounded 800 ms close — concurrent, inside the
    ///   §16.1 2 s window for the ack pump + WS flush). Finish errors
    ///   propagate — the show still stops but the ack is withheld, never a
    ///   silent ack. With both outputs live both are stopped; the first
    ///   failure wins the withheld ack.
    /// * `(_, true)`: immediate stop — the take is ABANDONED (file kept
    ///   as-is, no finish, no sidecar) and the stream session dropped as-is, a
    ///   warning is logged, the show stops.
    /// * `(false, false)` with a live output: refused `E_FORBIDDEN_STATE` —
    ///   told not to quiesce and not forced, so nothing is finalized and the
    ///   show keeps running.
    /// * A `Recording`/`Live` state with no session (the test seams) carries
    ///   no outputs and stops the show directly.
    ///
    /// [RI-8] unload-at-next-load: release decode sessions but retain package
    /// residency (video rings, image textures, audio assets) until the next
    /// show.load replaces it.
    async fn on_show_stop(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        self.on_show_stop_inner(d).await
    }

    /// `show.stop` implementation (async only so the stream quiescence can
    /// await the transport without blocking the executor; the record half is
    /// byte-identical behavior — no record-path change).
    async fn on_show_stop_inner(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        // Single-scope liveness read (TOCTOU note): each output's state +
        // session are read under one scope, session-then-state (the single lock
        // order — see `on_stream_stop`), so the pair cannot describe different
        // takes. A take landing after this still cannot corrupt: every quiesce
        // arm re-checks by taking the session (`None` = already gone = `Ok`).
        let (record_active, stream_active) = {
            let record_session = self.state.record_session.lock().unwrap();
            let record_state = self.state.record_state.lock().unwrap();
            let stream_session = self.state.stream_session.lock().unwrap();
            let stream_state = self.state.stream_state.lock().unwrap();
            (
                *record_state == RecordState::Recording && record_session.is_some(),
                *stream_state == StreamState::Live && stream_session.is_some(),
            )
        };
        if record_active || stream_active {
            let quiesce = payload_bool(&d.payload, "quiesceOutputs", true);
            let force = payload_bool(&d.payload, "force", false);
            match (quiesce, force) {
                (false, false) => {
                    return Err(DirectiveError::ForbiddenState(
                        "show.stop: quiesceOutputs=false with active outputs and no force".into(),
                    ));
                }
                (_, true) => {
                    if record_active {
                        *self.state.record_tap.lock().unwrap() = None;
                        if let Some(mut s) = self.state.record_session.lock().unwrap().take() {
                            s.abandon();
                        }
                        crate::record::markers::clear();
                        tracing::warn!(
                            "show.stop: force stop, recording abandoned as-is (no finish)"
                        );
                        *self.state.record_state.lock().unwrap() = RecordState::Idle;
                    }
                    if stream_active {
                        if let Some(mut s) = self.state.stream_session.lock().unwrap().take() {
                            s.abandon();
                        }
                        tracing::warn!("show.stop: force stop, stream abandoned as-is (no close)");
                        *self.state.stream_state.lock().unwrap() = StreamState::Idle;
                        *self.state.stream_tap_selection.lock().unwrap() = None;
                        *self.state.stream_tap.lock().unwrap() = None;
                    }
                }
                (true, false) => {
                    // Concurrent teardowns: the sequential worst case (1500 ms
                    // record + 800 ms stream = 2.3 s) exceeds the §16.1 2 s
                    // window, so the record finish runs on the blocking pool
                    // while the stream close awaits — first failure still wins
                    // the withheld ack, both arms still warn. The record body
                    // is byte-identical behavior, moved verbatim.
                    let handler = self.clone();
                    let record_join = tokio::task::spawn_blocking(move || {
                        handler.quiesce_record_for_show_stop(record_active)
                    });
                    let stream_result = if stream_active {
                        let mut session = self.state.stream_session.lock().unwrap().take();
                        let result = match session.as_mut() {
                            Some(s) => s.stop_and_close().await.map(|_| ()).map_err(stream_err),
                            None => Ok(()),
                        };
                        *self.state.stream_state.lock().unwrap() = StreamState::Idle;
                        // A stopped stream publishes no selection; the next
                        // start publishes fresh (see `on_stream_start`).
                        *self.state.stream_tap_selection.lock().unwrap() = None;
                        *self.state.stream_tap.lock().unwrap() = None;
                        match &result {
                            Ok(()) => {
                                info!("show.stop: stream quiesced");
                            }
                            Err(e) => {
                                tracing::warn!(err = %e, "show.stop: stream teardown failed");
                            }
                        }
                        Some(result)
                    } else {
                        None
                    };
                    let record_result = match record_join.await {
                        Ok(r) => r,
                        Err(e) => {
                            // A panicked/join-failed task is still a failure:
                            // route it through the shared cleanup path (stop
                            // clock, release sessions, clear transition) below
                            // rather than short-circuiting past it. Holds the
                            // error, does not return early.
                            Some(Err(DirectiveError::Invalid(format!(
                                "show.stop: record quiesce task failed: {e}"
                            ))))
                        }
                    };
                    // First failure wins. The show still stops, but the ack is
                    // withheld: a window with no graceful shutdown behind it.
                    // The transition clears on every exit from this arm, not
                    // just the Ok path — a failed finalize must not leave
                    // stale t0s for the next start either.
                    let first_err = record_result
                        .and_then(|r| r.err())
                        .or_else(|| stream_result.and_then(|r| r.err()));
                    if let Some(e) = first_err {
                        self.state.clock.lock().unwrap().stop();
                        self.state.sessions.release_all();
                        *self.state.transition.lock().unwrap() = None;
                        return Err(e);
                    }
                }
            }
        } else {
            if *self.state.record_state.lock().unwrap() == RecordState::Recording {
                // Session-less Recording (the test seam): no outputs to quiesce.
                *self.state.record_state.lock().unwrap() = RecordState::Idle;
            }
            if *self.state.stream_state.lock().unwrap() == StreamState::Live {
                // Session-less Live (the test seam): no outputs to quiesce.
                *self.state.stream_state.lock().unwrap() = StreamState::Idle;
            }
        }
        self.state.clock.lock().unwrap().stop();
        self.state.sessions.release_all();
        // A stopped show holds no transition: the clock restarts from zero on
        // the next start, so in-flight t0s would resume stale. Start-without-
        // load re-arms from a clean slate; load replaces state wholesale.
        *self.state.transition.lock().unwrap() = None;
        // Outputs are stubs in this prompt; the protocol shape is the point.
        Ok(())
    }

    /// The record half of `show.stop` graceful quiescence, on the blocking pool
    /// (see the concurrent-teardowns note in `on_show_stop_inner`): the bounded
    /// 1.5 s `stop_and_finish` wait never stalls the executor while the stream
    /// close awaits beside it. Body is the old sequential arm verbatim —
    /// `None` (take landed after the liveness read, or already gone) quiesces
    /// to `Ok`, exactly as before.
    fn quiesce_record_for_show_stop(
        &self,
        record_active: bool,
    ) -> Option<Result<(), DirectiveError>> {
        if !record_active {
            return None;
        }
        *self.state.record_tap.lock().unwrap() = None;
        let mut session = self.state.record_session.lock().unwrap().take();
        let result = match session.as_mut() {
            Some(s) => s
                .stop_and_finish(crate::record::RECORD_STOP_TIMEOUT)
                .map(|_| ())
                .map_err(session_err),
            None => {
                crate::record::markers::clear();
                Ok(())
            }
        };
        *self.state.record_state.lock().unwrap() = RecordState::Idle;
        match &result {
            Ok(()) => {
                crate::record::markers::clear();
                info!("show.stop: recording quiesced");
            }
            Err(DirectiveError::Timeout(_)) => {
                tracing::warn!(
                    "show.stop: graceful record shutdown timed out; take force-abandoned, file kept as-is"
                );
            }
            Err(e) => {
                crate::record::markers::clear();
                tracing::warn!(err = %e, "show.stop: recording finalize failed");
            }
        }
        Some(result)
    }

    fn on_take(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        let item_ref = d
            .target
            .get("itemRef")
            .and_then(|v| v.as_str())
            .or_else(|| d.target.get("sceneId").and_then(|v| v.as_str()));
        if let Some(r) = item_ref {
            // Prompt 04 Step 2: the transition the control plane already
            // resolved (SPEC §16.2 — never re-resolved here) takes effect on
            // the NEXT frame boundary, never mid-frame.
            let previous = self.state.view_item.lock().unwrap().clone();
            let kind = match d.payload.get("transition").and_then(|v| v.as_str()) {
                Some("mix") => crate::scene::TransitionKind::Mix,
                _ => crate::scene::TransitionKind::Cut,
            };
            let transition_frames = d
                .payload
                .get("durationFrames")
                .and_then(|v| v.as_u64())
                .unwrap_or(if kind == crate::scene::TransitionKind::Mix {
                    15
                } else {
                    0
                });
            let start_frame = self.state.master_frame().map(|f| f + 1).unwrap_or(0);
            // Step 1 mid-mix rule. UNRATIFIED spec-correction candidate: a
            // PROPOSED NEW ROW for §17.3 (draft, NOT in the spec file — §17.3
            // today has no mid-transition row at all: every row starts from a
            // steady item state (READY/ARMED/LIVE/PLAYING/...) and no row
            // covers a take landing while a `mix` is still in flight on the
            // same bus. That absence is the point of the proposal):
            //
            //   "A take whose transition is `mix`, landing while a `mix` is
            //    still in flight on the same bus, starts from the currently
            //    displayed blended state: the interrupted transition's two
            //    layers (from@1.0, to@α_frozen, α frozen at the last blended
            //    frame) composite beneath the new to_item at its fresh α,
            //    and the underlay is dropped when the new transition
            //    completes. A take whose transition is `cut` keeps instant
            //    semantics mid-mix (a cut is supposed to snap). Boundary
            //    discipline is unchanged on every path: start_frame =
            //    master+1, never mid-frame."
            //
            // Mechanism choice: freeze-at-take (this site) + composite in
            // `scene_for`'s mix branch, rather than flattening pixels. The
            // underlay keeps item refs with their own t0s, so video timelines
            // (§12.1) and the completed-mix drop read the same clocks as any
            // ordinary mix; freezing pixels would have forked a second,
            // unclocked picture path. Chained interrupts EXTEND the flat
            // underlay by one frozen layer instead of nesting: the visible
            // frame IS the flattened top blend, and the nested Transition
            // shells beneath it carry nothing the render path can reach (see
            // `scene::Underlay`), so they are dropped, not wrapped.
            let frozen_frame = start_frame.saturating_sub(1);
            let previous_transition = self.state.transition.lock().unwrap().clone();
            let underlay = match kind {
                crate::scene::TransitionKind::Mix => match &previous_transition {
                    Some(old)
                        if old.kind == crate::scene::TransitionKind::Mix
                            && !old.is_complete(frozen_frame) =>
                    {
                        // Collapse: reuse the already-frozen layers verbatim
                        // (they ARE the displayed composite's base), then
                        // append the outgoing transition's to_item at its
                        // frozen α. When the old transition has no underlay
                        // its from_item is the genuine base; when it HAS one
                        // its from_item is stale (see the construction note
                        // below) and the frozen layers already cover it.
                        let mut layers = match &old.underlay {
                            Some(u) => u.layers.clone(),
                            None => old
                                .from_item
                                .as_deref()
                                .map(|from| crate::scene::FrozenLayer {
                                    item: from.to_string(),
                                    alpha: 1.0,
                                    t0: old.from_start_frame,
                                })
                                .into_iter()
                                .collect(),
                        };
                        // Back-to-back take (old progress still 0.0): pushing the
                        // to_item would carry a dead zero-alpha layer for the
                        // whole new mix, so skip it. If nothing remains (no
                        // from_item either — the interrupted transition had
                        // rendered nothing yet), there is no underlay at all
                        // and the new mix starts clean, exactly as a fresh
                        // take would.
                        let frozen = old.progress(frozen_frame);
                        if frozen > 0.0 {
                            layers.push(crate::scene::FrozenLayer {
                                item: old.to_item.clone(),
                                alpha: frozen,
                                t0: old.start_frame,
                            });
                        }
                        // Bound: every chained interrupt appends exactly one
                        // layer, so adversarial take rates grow the composite
                        // linearly (and squeeze the shared MAX_LAYERS draw
                        // budget overlays rely on). Past the cap, drop the
                        // oldest NON-base layer — the base (index 0) is the
                        // continuity anchor and is never the one to go. The
                        // cap sits far above human rates (WS dispatch admits
                        // ~5 takes/s; a 20-frame mix spans <1 s), so it fires
                        // only as insurance, and the drop is logged loudly.
                        const MAX_UNDERLAY_LAYERS: usize = 8;
                        while layers.len() > MAX_UNDERLAY_LAYERS {
                            layers.remove(1);
                            tracing::warn!(
                                "underlay layer cap hit; dropping oldest non-base frozen layer"
                            );
                        }
                        (!layers.is_empty()).then_some(crate::scene::Underlay { layers })
                    }
                    _ => None,
                },
                // A cut landing mid-mix snaps: overwrite, no underlay (GAP-8).
                crate::scene::TransitionKind::Cut => None,
            };
            // SPEC §12.1: the incoming item's timeline starts at the frame it
            // goes on air, and the outgoing item keeps reading from its own
            // start for the length of a mix.
            let previous_start = self
                .state
                .view_item_start_frame
                .load(std::sync::atomic::Ordering::SeqCst);
            *self.state.transition.lock().unwrap() = Some(crate::scene::Transition {
                // STALE-UNDER-UNDERLAY (harmless by construction): while the
                // new transition carries an `underlay`, `scene_for` composites
                // the frozen layers and never reads this — it names the old
                // to_item, not the displayed blend — and after the transition
                // completes the whole struct is ignored until the next take
                // overwrites it. Recorded for the plain (no-underlay) mix
                // path, which does read it.
                from_item: previous,
                from_start_frame: previous_start,
                to_item: r.to_string(),
                kind,
                duration_frames: transition_frames,
                start_frame,
                underlay,
            });
            *self.state.view_item.lock().unwrap() = Some(r.to_string());
            self.state
                .view_item_start_frame
                .store(start_frame, std::sync::atomic::Ordering::SeqCst);

            // SPEC §8.7.3: the take's audio object decides what the clip bus
            // does. `follow` takes the item's own audioPolicy (AFV).
            let mode = d
                .payload
                .get("audio")
                .and_then(|a| a.get("transition"))
                .and_then(|v| v.as_str())
                .unwrap_or("follow");
            let ramp_ms = d
                .payload
                .get("audio")
                .and_then(|a| a.get("rampMs"))
                .and_then(|v| v.as_f64())
                .unwrap_or(10.0) as f32;
            // §8.7.5: a video mix crossfades audio over the same duration.
            // §8.7.6: a video cut still ramps, never steps.
            let crossfade_frames = if kind == crate::scene::TransitionKind::Mix {
                d.payload
                    .get("audio")
                    .and_then(|a| a.get("durationFrames"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(transition_frames)
            } else {
                0
            };
            self.state.audio_commands.lock().unwrap().push(
                // Audio mid-mix honesty: a take REPLACES the clip-bus source —
                // it restarts through silence via `swap_source_through_silence`
                // — rather than crossfading out of the current video blend.
                // So a mid-mix take queues a FRESH TakeItem for the new item
                // at the new t0 (below), never a continuation of the
                // in-flight one; the graph drops the old source first and the
                // new item's audio starts clean.
                crate::audio_control::AudioCommand::TakeItem {
                    item_ref: r.to_string(),
                    // Resolved from the map built at load. `None` is a
                    // legitimate answer — a graphic-only scene has no audio —
                    // and is distinct from "the lookup missed", which is what
                    // used to happen silently.
                    asset_id: self.state.item_audio.lock().unwrap().get(r).cloned(),
                    t0: start_frame,
                    mode: mode.to_string(),
                    ramp_ms,
                    crossfade_frames,
                },
            );
            let generation = self.playing.begin(r);
            if let Some(frames) = duration_frames(d) {
                self.schedule_done(r.to_string(), frames, generation);
            }
        }
        Ok(())
    }

    /// Audio directives (SPEC §16.8). The graph lives on the audio thread; the
    /// directive path only publishes intent, which is why this never blocks.
    fn on_audio(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        let mut pending = self.state.audio_commands.lock().unwrap();
        pending.push(crate::audio_control::AudioCommand::from_directive(d)?);
        Ok(())
    }

    /// Output commands, WU-pipe unified (SPEC §16.14): `record.start` carries
    /// `{ outputId? }` ONLY. Show, episode, geometry, and rate derive from the
    /// loaded package + engine — the retired `show`/`episode`/`width`/
    /// `height`/`fps`/`startTimestamp` payload fields are NOT read (extra
    /// fields are ignored, never rejected, so an older control plane keeps
    /// working while its naming fields stop mattering). `outputId` selects
    /// the (single) configured record target and names the episode filename
    /// component; the show component reads the package manifest
    /// (`show.title`, else `show.id`); geometry is the View (`VIEW_W/H` — the
    /// loop's ONE encoder records what is on air); rate is the house rate;
    /// the timestamp is generated.
    ///
    /// The start path PROBES for a hardware encoder without opening a stream
    /// (SPEC precondition); the recording encoder opens later, on the record
    /// thread. The handler stays non-blocking: session open only reserves the
    /// path and spawns the thread.
    ///
    /// Refusals: `E_FORBIDDEN_STATE` when the show is not RUNNING or a
    /// recording is already active (the refused second start preserves the
    /// live pipeline but still resets chapters for the next take);
    /// `Invalid` when no record directory is configured or the path cannot be
    /// reserved (`E_DISK` inside); `E_DISK` early when the target is
    /// unwritable/full (admission probe, before the take owns anything);
    /// `E_UNSUPPORTED` when the record geometry exceeds 1920x1080 (see
    /// [`check_record_resolution`]); `E_NO_HARDWARE_ENCODER` when the probe
    /// fails (forced or genuine). A successful start reserves the path, spawns
    /// the thread, zeroes `record_tap_ms` + `skipped_record_frames`, and flips
    /// to Recording.
    fn on_record_start(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        if !self.state.is_running() {
            return Err(DirectiveError::ForbiddenState(
                "record.start requires a running show".into(),
            ));
        }
        // Fresh marker list per recording: a new take never inherits the
        // previous take's chapters. Resets on every start attempt while the
        // show runs — including a refused second start — and only there
        // (never while stopped).
        crate::record::markers::clear();
        // Defined behavior: a second start while Recording is refused — the
        // live pipeline is preserved.
        if *self.state.record_state.lock().unwrap() == RecordState::Recording {
            return Err(DirectiveError::ForbiddenState(
                "record.start while already recording".into(),
            ));
        }
        let dir = self.state.record_dir.lock().unwrap().clone();
        let Some(dir) = dir else {
            return Err(DirectiveError::Invalid(
                "record.start: no record directory (show.outputs.record.directory not configured)"
                    .into(),
            ));
        };
        // Resolution ceiling: the loop records what is on air (VIEW_W/H).
        check_record_resolution(VIEW_W, VIEW_H)?;
        // Admission: refuse early on an unwritable/full target (E_DISK here,
        // not mid-take). The session open would refuse a missing path on its
        // own, but only this probe measures free space before the take owns it.
        crate::record::available_space_mib(&dir).map_err(finish_err)?;
        // SPEC §16.14 precondition, wired (no dead variant): probe, no stream.
        if !crate::record::session::encoder_available() {
            return Err(DirectiveError::NoHardwareEncoder(
                "record.start: no hardware H.264 encoder available".into(),
            ));
        }
        let episode = d
            .payload
            .get("outputId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("episode")
            .to_string();
        let show = record_show_name(&self.state);
        let fps = self.state.house_rate();
        let start_timestamp = default_timestamp();
        // The shared tap: published here, attached to the live graph by the
        // audio driver, drained by the record thread.
        let tap = Arc::new(crate::record::AudioTap::new());
        let session = crate::record::RecordSession::open(
            &dir,
            &show,
            &episode,
            &start_timestamp,
            VIEW_W,
            VIEW_H,
            fps,
            tap.clone(),
            self.state.skipped_record_frames.clone(),
        )
        .map_err(finish_err)?;

        // ZERO-COPY Phase 3b, step 5 — THE MIGRATION. This is the first point
        // at which telemetry's claim and the frame path's behaviour are the
        // same statement: the path is probed, chosen by the published table,
        // published for telemetry, and built, all here.
        //
        // The probe is honest: it builds the whole chain at the take's geometry
        // and keeps it (as the pool) or fails. A `None` device — a headless
        // engine, or a build where the render loop has not run — is a machine
        // with no chain, and the table's answer for that is `CpuReadback` /
        // `ProbeUnavailable`, which is a lawful take under the v0.4.2
        // allowance and not a failure.
        let mut session = session;
        // WU2 — the manifest's `outputs.record.tapPath` override (SPEC
        // v0.4.5). `auto` (or an absent/unreadable field) leaves the table
        // speaking; `cpuReadback` restricts the take to CPU and reports
        // `Override`. A restricted take builds no pool: the shared take pool
        // (G1 sizing — record bound + stream bound + drawn, five 1080p
        // surfaces) is VRAM the take must not use. Alone that saves the pool;
        // alongside a live stream the loop still holds the stream's own
        // same-sized pool (the stream leg falls back to its own surface when
        // the CPU take has none to share), so the restriction redirects who
        // the surfaces serve rather than saving them. `None` here is exactly
        // `select`'s behavior, so takes without the field are bit-identical
        // to before.
        let override_path = record_tap_override(&self.state);
        let device = self.state.render_device();
        let pool = if override_path == Some(crate::record::tap_path::TapPath::CpuReadback) {
            None
        } else {
            device.and_then(|d| {
                match crate::record::shared_zerocopy_pool(&d) {
                    Ok(p) => Some(Arc::new(p)),
                    Err(e) => {
                        // Loud, because a silent fallback to the readback path is
                        // exactly the event `record_tap_reason` exists to expose.
                        tracing::warn!(err = %e, "record.start: zero-copy unavailable, falling back to CPU readback");
                        None
                    }
                }
            })
        };
        let selection = crate::record::tap_path::select_with_override(
            pool.is_some(),
            VIEW_H,
            crate::record::tap_path::Consumer::Record,
            override_path,
        );
        if let Some(pool) = pool {
            session.set_surface_pool(pool);
        }
        info!(
            path = selection.path.as_str(),
            reason = ?selection.reason,
            "record.start: frame path selected"
        );
        // Published for the §10.1 tick. Not cleared at stop: the field is per
        // take and reads as the path the LAST take used, which is the answer an
        // operator asking "what did that take do?" needs. `"none"` stays the
        // answer only for a machine that has never recorded.
        *self.state.record_tap_selection.lock().unwrap() = Some(selection);

        // Fresh counters per take: the loop accumulates into these for the
        // take's lifetime, so a new take starts from zero (set before the
        // state flip, while the loop still sees Idle).
        *self.state.record_tap_ms.lock().unwrap() = 0.0;
        self.state.skipped_record_frames.store(0, Ordering::SeqCst);
        *self.state.record_tap.lock().unwrap() = Some(tap);
        *self.state.record_session.lock().unwrap() = Some(session);
        *self.state.record_state.lock().unwrap() = RecordState::Recording;
        Ok(())
    }

    /// `record.stop` ends the take gracefully: signal + bounded wait for the
    /// record thread — the file + sidecar are complete before `apply()` emits
    /// the ack — then returns the state machine to Idle. Requires an active
    /// recording (`E_FORBIDDEN_STATE` while Idle). A session-less `Recording`
    /// (the test seam) flips with nothing to finish and preserves the marker
    /// store: only the session owner (the thread's finish/abandon paths and
    /// the successful-stop path below) clears markers, so a second stop that
    /// finds no session cannot empty the take's chapters. A failed finish
    /// still ends the recording (no pipeline remains to continue with) but
    /// withholds the ack: `apply()` only acks on `Ok`. A timed-out wait takes
    /// the force path (file kept as-is, warning logged) and likewise withholds
    /// the ack.
    ///
    /// Concurrency note: the state check and the session take hold the session
    /// guard continuously (one acquisition), so two concurrent stops cannot
    /// both take the session — the loser finds `None` and clears nothing. Two
    /// separate mutexes (`record_state`, `record_session`) cannot be acquired
    /// atomically, so the residual is documented, not eliminated: a stop that
    /// observes `Recording` while a start is still publishing may take `None`
    /// and flip to Idle just as the start publishes its session, orphaning it
    /// (its thread still exits via the frames-gone path; the file stays
    /// as-is). In practice the loop never issues concurrent record stops (one
    /// directive stream) and starts/stops are sequential. What IS
    /// deterministic — a session-less stop preserves markers — is pinned by
    /// test; the rest is documented here.
    fn on_record_stop(&self, _d: &DirectiveFrame) -> Result<(), DirectiveError> {
        // Hold the session guard across the state check + take: one
        // acquisition, so a concurrent stop cannot interleave between them.
        // The guard is dropped before the blocking wait below.
        let mut session_guard = self.state.record_session.lock().unwrap();
        if *self.state.record_state.lock().unwrap() != RecordState::Recording {
            return Err(DirectiveError::ForbiddenState(
                "record.stop requires an active recording".into(),
            ));
        }
        // Detach the driver first: the thread keeps its own Arc for the tail
        // drain, and no new live mix enters the take after this point.
        *self.state.record_tap.lock().unwrap() = None;
        // Taken, not borrowed: the wait runs without holding any state lock.
        let mut session = session_guard.take();
        drop(session_guard);
        // Ownership decides clearing: only the session owner clears markers.
        let owned = session.is_some();
        let result = match session.as_mut() {
            Some(s) => s
                .stop_and_finish(crate::record::RECORD_STOP_TIMEOUT)
                .map(|_| ())
                .map_err(session_err),
            // No session to own the take: flip to Idle, clear nothing. Only
            // the session owner clears markers (thread finish/abandon paths,
            // successful stop below); the next start/load clears anyway.
            None => Ok(()),
        };
        *self.state.record_state.lock().unwrap() = RecordState::Idle;
        match &result {
            Ok(()) if owned => {
                crate::record::markers::clear();
                info!("record.stop: take finished");
            }
            Ok(()) => {
                info!("record.stop: session-less stop; markers preserved");
            }
            Err(DirectiveError::Timeout(_)) => {
                // The detached take's late finish owns its marker snapshot —
                // clearing here would empty its sidecar — so only the next
                // start/load clears.
                tracing::warn!(
                    "record.stop: graceful shutdown timed out; take force-abandoned, file kept as-is"
                );
            }
            Err(e) => {
                crate::record::markers::clear();
                tracing::warn!(err = %e, "record.stop: take finalize failed");
            }
        }
        result
    }

    /// Output commands, WU4 (SPEC §16.14): `stream.start` carries
    /// `{ outputId?, url? }`. The endpoint resolves per WU3
    /// ([`resolve_stream_url`]): the command's `url` overrides the manifest's
    /// `show.outputs.stream.url` for the run; neither present is refused
    /// `E_BAD_PAYLOAD`-shaped (before the encoder and chain probes, so that
    /// refusal is hardware-free).
    ///
    /// Preconditions, in order — **decided deliberately (PR #30 repair round)
    /// and pinned by `stream_start_refusal_order_is_config_then_chain_then_encoder`**:
    ///
    /// 1. show RUNNING (`E_FORBIDDEN_STATE`);
    /// 2. no live stream already (`E_FORBIDDEN_STATE` — §9.1's exactly-one-live
    ///    ceiling; the live session is preserved);
    /// 3. endpoint resolved (`E_BAD_PAYLOAD`);
    /// 4. the manifest's `outputs.stream.tapPath` is not `cpuReadback`
    ///    (`E_NO_ZEROCOPY`) — a configuration refusal, so it is decided before
    ///    any probe and is the same answer on every machine;
    /// 5. a zero-copy chain is available (`E_NO_ZEROCOPY`, loudly — v0.4.4's
    ///    ratified rescope gives the readback allowance to recording only);
    /// 6. a hardware encoder answers the SPEC probe (`E_NO_HARDWARE_ENCODER`).
    ///
    /// **Why this order.** ~~The chain refusal is the SPEC's claim — "this
    /// output has no lawful path on this machine" — and the encoder refusal
    /// is this build's. The law's answer is reported first.~~ That was wrong
    /// (§2c): §9.2's hardware-only encode is spec law too, so both refusals
    /// come from the spec, and §16.14 lists its preconditions without an
    /// evaluation order (encoder first, as it happens). The order rests on
    /// two honest grounds instead. A configuration refusal (`cpuReadback`) is
    /// the same answer on every machine, so it is decided before any probe.
    /// And config → chain → encoder is the only order the CI runner — a Metal
    /// adapter, no H.264 encoder — can observe: with the encoder probe first,
    /// every chain and configuration refusal was unreachable there; the first
    /// version of this function shipped that way and
    /// `stream_tap_path_cpu_readback_is_refused_no_zerocopy` failed on CI (run
    /// 35878301689) while passing on a machine with an encoder. ~~Because a
    /// test now pins an order §16.14 never states, the order is drafted as an
    /// UNRATIFIED candidate in `docs/v0.5-outline.md` §7.~~ **§16.14 states
    /// this order as law since v0.4.6** (ratified 2026-09-25, on these two
    /// grounds and no others); the test is its guard.
    ///
    /// A successful start opens the [`crate::record::stream::StreamSession`],
    /// publishes the selection (`select_stream`'s gate, then
    /// `select_with_override` with the manifest's `outputs.stream.tapPath` —
    /// the B2 stream side, record-WU2 shape), and flips to Live. The transport
    /// dials in WU5; WU4 owns the bookkeeping the ack waits for.
    fn on_stream_start(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        if !self.state.is_running() {
            return Err(DirectiveError::ForbiddenState(
                "stream.start requires a running show".into(),
            ));
        }
        // Defined behavior: a second start while Live is refused — the live
        // session is preserved (§9.1 ceiling: exactly one live stream).
        if *self.state.stream_state.lock().unwrap() == StreamState::Live {
            return Err(DirectiveError::ForbiddenState(
                "stream.start while already live".into(),
            ));
        }
        // A non-string `url` is a malformed endpoint (E_BAD_PAYLOAD), never
        // silent: falling back to the manifest would publish somewhere the
        // operator did not name. Missing/null/empty stays silent per WU3.
        let command_url = match d.payload.get("url") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.as_str()),
            Some(_) => {
                return Err(DirectiveError::Invalid(
                    "E_BAD_PAYLOAD: stream.start url must be a string".into(),
                ));
            }
        };
        // One manifest read for the whole start: url, tapPath, bitrates.
        let output = stream_manifest_output(&self.state);
        // WU3 call site: the manifest answers when the command is silent.
        let manifest_url = output.as_ref().and_then(|o| o.url.as_deref());
        let endpoint = resolve_stream_url(manifest_url, command_url)?;
        // (4) B2 stream side — the manifest's `outputs.stream.tapPath` override
        // (SPEC v0.4.5). A `cpuReadback` restriction is unlawful for streaming
        // (no readback allowance covers this output) and is refused on the
        // `E_NO_ZEROCOPY` path — never an Override-live selection. A
        // CONFIGURATION refusal: decided before any probe, identical on every
        // machine.
        let override_path = stream_tap_override(output.as_ref());
        if override_path == Some(crate::record::tap_path::TapPath::CpuReadback) {
            tracing::warn!("stream.start: cpuReadback override refused (no lawful readback path for streaming)");
            return Err(stream_err(
                crate::record::stream::StreamError::NoChain(
                    "stream.start refused: cpuReadback tapPath is unlawful for streaming; \
                     streaming shares frames with the encoder without CPU readback (§0.1 assumption 24)"
                        .into(),
                ),
            ));
        }
        // (5) The refusal row: no chain, no lawful path (never a silent
        // fallback to the readback the spec forbids this output). Answered
        // before the encoder probe for the reasons in the doc comment above
        // (~~"the SPEC's claim, so it is answered before this build's encoder
        // probe"~~ — both are the spec's). The probe IS the stream's pool:
        // kept on success, owned by the session.
        let pool = crate::record::stream::probe_stream_pool(&self.state.render_device());
        let capable = pool.is_some();
        if crate::record::tap_path::select_stream(capable).is_none() {
            tracing::warn!("stream.start: zero-copy chain unavailable, refusing (no readback fallback for streaming)");
            return Err(stream_err(
                crate::record::stream::StreamError::NoChain(
                    "stream.start refused: machine has no zero-copy chain; \
                     streaming shares frames with the encoder without CPU readback (§0.1 assumption 24)"
                        .into(),
                ),
            ));
        }
        // (6) SPEC §16.14 precondition, wired: a hardware H.264 encoder
        // (§9.2 — spec law, like the chain; ~~"this build's hardware
        // encoder"~~ was the old refusal-order framing, corrected above).
        if !crate::record::session::encoder_available() {
            return Err(DirectiveError::NoHardwareEncoder(
                "stream.start: no hardware H.264 encoder available".into(),
            ));
        }
        let selection = crate::record::tap_path::select_with_override(
            capable,
            VIEW_H,
            crate::record::tap_path::Consumer::Stream,
            override_path,
        );
        info!(
            path = selection.path.as_str(),
            reason = ?selection.reason,
            "stream.start: frame path selected"
        );
        // Published for the §10.1 tick's stream side (WU5 wires the field; the
        // shape is the record `record_tap_selection` one). Cleared on every
        // stop path: a stopped stream publishes no selection, and the next
        // start publishes fresh.
        *self.state.stream_tap_selection.lock().unwrap() = Some(selection);
        // Fresh counters per stream: the loop accumulates into these for the
        // stream's lifetime, so a new stream starts from zero (set before the
        // state flip, while the loop still sees Idle) — the record-start shape.
        *self.state.stream_tap_ms.lock().unwrap() = 0.0;
        self.state.skipped_stream_frames.store(0, Ordering::SeqCst);
        // §9.4 at the show's geometry and rate, with the manifest's bitrates.
        let params = crate::record::stream::StreamParams::new(
            crate::render::VIEW_W,
            VIEW_H,
            self.state.house_rate(),
        )
        .with_output(output.as_ref());
        // Spawns the publisher (it dials in the background) and the stream
        // thread (it opens the encoders on itself) — nothing here waits on
        // either.
        let mut session = crate::record::stream::StreamSession::open(
            endpoint,
            selection,
            params,
            self.state.skipped_stream_frames.clone(),
        );
        if let Some(pool) = pool {
            session.set_surface_pool(Arc::new(pool));
        }
        // The audio driver attaches the stream's tap on its next cycle.
        *self.state.stream_tap.lock().unwrap() = Some(session.tap());
        *self.state.stream_session.lock().unwrap() = Some(session);
        *self.state.stream_state.lock().unwrap() = StreamState::Live;
        Ok(())
    }

    /// `stream.stop` closes the live session BEFORE `apply()` emits the ack —
    /// the record `stop_and_finish` shape. Requires a live stream
    /// (`E_FORBIDDEN_STATE` while Idle). A session-less `Live` (the test seam)
    /// flips with nothing to close. A failed teardown still ends the live
    /// state (no pipeline remains to continue with) but withholds the ack:
    /// `apply()` only acks on `Ok`.
    ///
    /// Concurrency note: the state check and the session take hold the session
    /// guard continuously (one acquisition) — the record `on_record_stop`
    /// shape — so two concurrent stops cannot both take the session.
    /// Async so the bounded transport wait (tokio sleep) never blocks the
    /// executor — `stream.stop` must not stall the ack window (bounded-wait
    /// assertion in `tests/prompt10_rtmp.rs`).
    async fn on_stream_stop(&self, _d: &DirectiveFrame) -> Result<(), DirectiveError> {
        // Take the session under one short sync scope (state check + take
        // atomically, so concurrent stops cannot interleave); the guard is
        // dropped BEFORE the await below, so this future stays Send and the
        // executor never blocks on a std MutexGuard.
        //
        // Lock order (global, session before state — never state-then-session):
        // the session guard is acquired first and the state read nests inside
        // it. The feed leg (`main.rs`) holds the session only across bounded
        // `try_send`s and never nests a state lock inside, so a stop racing
        // the feed waits boundedly instead of deadlocking.
        let mut session = {
            let mut session_guard = self.state.stream_session.lock().unwrap();
            if *self.state.stream_state.lock().unwrap() != StreamState::Live {
                return Err(DirectiveError::ForbiddenState(
                    "stream.stop requires a live stream".into(),
                ));
            }
            // Taken, not borrowed: the close runs without holding any state lock.
            session_guard.take()
        };
        let result = match session.as_mut() {
            Some(s) => s.stop_and_close().await.map(|_| ()).map_err(stream_err),
            // No session to own the close: flip to Idle, close nothing.
            None => Ok(()),
        };
        *self.state.stream_state.lock().unwrap() = StreamState::Idle;
        *self.state.stream_tap_selection.lock().unwrap() = None;
        *self.state.stream_tap.lock().unwrap() = None;
        match &result {
            Ok(()) => {
                info!("stream.stop: session closed");
            }
            Err(e) => {
                tracing::warn!(err = %e, "stream.stop: teardown failed");
            }
        }
        result
    }

    /// `marker.add` (SPEC §16.11, `[RI-5]`): requires an active recording —
    /// while Idle the marker would be recorded nowhere, so accept-but-ignore
    /// is silent loss and is refused with `E_FORBIDDEN_STATE` instead. While
    /// Recording the marker lands in the process-wide
    /// [`crate::record::markers`] store with the master frame; the sidecar
    /// writer snapshots it at finish.
    /// Frame rule: `master_frame() + 1` — the marker takes effect on the NEXT
    /// frame boundary, the same discipline as a take (`on_take`) and an
    /// overlay (`on_overlay`), never mid-frame.
    /// Timecode rule: the verbatim `timecode` string is kept as given; `frame`
    /// is the authority for ordering/chapters.
    fn on_marker_add(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        if *self.state.record_state.lock().unwrap() != RecordState::Recording {
            return Err(DirectiveError::ForbiddenState(
                "marker.add requires an active recording".into(),
            ));
        }
        let name = d
            .payload
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| DirectiveError::Invalid("marker.add missing name".into()))?;
        let timecode = d
            .payload
            .get("timecode")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        crate::record::markers::add(crate::record::markers::Marker {
            name: name.to_string(),
            frame: self.state.master_frame().map(|f| f + 1).unwrap_or(0),
            timecode,
        });
        Ok(())
    }

    fn on_fallback(&self, _d: &DirectiveFrame) -> Result<(), DirectiveError> {
        self.state.fallback_active.store(true, Ordering::SeqCst);
        *self.state.view_item.lock().unwrap() = None; // on fallback, view shows the slate
        Ok(())
    }

    /// Overlay show/hide (SPEC §7.10). The control plane already validated the
    /// overlay against the package; the engine is lenient and tracks on-air
    /// state only. The animation keys off the master clock at the frame *after*
    /// the command lands — the same boundary discipline as a take — and a take
    /// never touches these timelines.
    fn on_overlay(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        let Some(overlay_id) = d
            .target
            .get("overlayId")
            .and_then(|v| v.as_str())
            .or_else(|| d.payload.get("overlayId").and_then(|v| v.as_str()))
        else {
            return Ok(());
        };
        let show = d.command == "overlay.show";
        // The declared enter/exit duration lives in the loaded package index;
        // without one, a single frame is the honest fallback (no animation).
        // payload.animation.durationFrames, when present, overrides the package
        // bound for that show/hide. Easing and delayFrames, if carried, are
        // ignored: overlay animations are linear alpha ramps (see records doc
        // entry c).
        let override_frames = d
            .payload
            .get("animation")
            .and_then(|a| a.get("durationFrames"))
            .and_then(|v| v.as_u64())
            .filter(|v| *v >= 1);
        let frames = if let Some(o) = override_frames {
            o
        } else {
            let pkg = self.state.package.lock().unwrap();
            match pkg.as_ref() {
                Some(idx) if show => idx
                    .overlay_enter_frames
                    .get(overlay_id)
                    .copied()
                    .unwrap_or(1),
                Some(idx) => idx
                    .overlay_exit_frames
                    .get(overlay_id)
                    .copied()
                    .unwrap_or(1),
                None => 1,
            }
        };
        // Read the clock before locking `overlays` so no lock is held across
        // the clock acquire (keeps the lock order clock-after-overlays
        // direction failed-friendlier).
        let next_frame = self.state.master_frame().map(|f| f + 1).unwrap_or(1);
        let mut overlays = self.state.overlays.lock().unwrap();
        match (show, overlays.get(overlay_id).copied()) {
            // Show on an on-air overlay is a no-op only when it is Steady or
            // Entering. A show during an Exit revives the overlay: the control
            // plane re-adds and forwards (its `visibleOverlays` was already
            // deleted on hide), so the engine must flip it back to Enter rather
            // than let it drop at exit-complete — otherwise the two layers
            // disagree about the overlay's fate.
            (true, Some(ov)) if ov.on_air && ov.phase != crate::state::OverlayPhase::Exit => {}
            (false, Some(ov)) if ov.phase == crate::state::OverlayPhase::Exit => {} // hide already hiding
            (false, None) => {} // hide on hidden: idle no-op
            (true, _) => {
                overlays.insert(
                    overlay_id.to_string(),
                    crate::state::OverlayRuntime {
                        on_air: true,
                        anim_start: next_frame,
                        duration_frames: frames,
                        phase: crate::state::OverlayPhase::Enter,
                    },
                );
            }
            (false, Some(_)) => {
                overlays.insert(
                    overlay_id.to_string(),
                    crate::state::OverlayRuntime {
                        on_air: true,
                        anim_start: next_frame,
                        duration_frames: frames,
                        phase: crate::state::OverlayPhase::Exit,
                    },
                );
            }
        }
        Ok(())
    }

    fn on_resync(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        let snapshot = &d.payload;
        let show_state = snapshot
            .get("showState")
            .and_then(|v| v.as_str())
            .unwrap_or("STOPPED");
        match show_state {
            "RUNNING" => self.state.clock.lock().unwrap().start(),
            _ => self.state.clock.lock().unwrap().stop(),
        }
        if snapshot.get("fallbackActive").and_then(|v| v.as_bool()) == Some(true) {
            self.state.fallback_active.store(true, Ordering::SeqCst);
        }
        // The snapshot is authoritative about BOTH buses, including when a bus
        // is empty. Reading only the naming case left the previous item on air
        // after a show.stop resync said nothing was.
        let now = self.state.master_frame().unwrap_or(0);
        if snapshot.get("viewItem").is_some() {
            let view = snapshot
                .get("viewItem")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            *self.state.view_item.lock().unwrap() = view;
            // SPEC §5.9.4 (v0.4): the snapshot carries `viewItemStartFrame` —
            // §12.1's `t0` — so a resynced timed item resumes where it
            // actually is. v0.3's snapshot said WHAT was on air but not SINCE
            // WHEN, so this guessed `now`, and a clip forty seconds in jumped
            // back to zero on air. Falling back to `now` when the field is
            // absent keeps a v0.3 control plane working, badly, rather than
            // not at all.
            let t0 = snapshot
                .get("viewItemStartFrame")
                .and_then(|v| v.as_u64())
                .unwrap_or(now);
            self.state
                .view_item_start_frame
                .store(t0, std::sync::atomic::Ordering::SeqCst);
        }
        // A resync supersedes any transition the engine was mid-way through:
        // the snapshot is the state, not a waypoint toward it. Unconditional —
        // an absent viewItem key means an empty bus, which holds no transition
        // either (and the old key-gated placement stranded one there).
        *self.state.transition.lock().unwrap() = None;
        if snapshot.get("previewItem").is_some() {
            let preview = snapshot
                .get("previewItem")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            *self.state.preview_item.lock().unwrap() = preview;
            self.state
                .preview_item_start_frame
                .store(now, std::sync::atomic::Ordering::SeqCst);
        }
        // §5.9.4 (v0.4, implemented here per step 5's mandate): `visibleOverlays`
        // is a full snapshot, not a patch. A present array — including an empty
        // one — replaces the on-air set wholesale; an absent key leaves it alone.
        if let Some(visible) = snapshot.get("visibleOverlays").and_then(|v| v.as_array()) {
            let ids: Vec<String> = visible
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let mut overlays = self.state.overlays.lock().unwrap();
            overlays.clear();
            for id in ids {
                // Resynced overlays are authoritative state, not an animation:
                // they land steady.
                overlays.insert(
                    id,
                    crate::state::OverlayRuntime {
                        on_air: true,
                        anim_start: now,
                        duration_frames: 1,
                        phase: crate::state::OverlayPhase::Steady,
                    },
                );
            }
        }
        self.state.set_last_applied(d.state_version);
        info!(sv = d.state_version, "show.resync applied");
        Ok(())
    }

    /// The Section 5.9.5 ack: emitted only after the engine has executed the
    /// quiesce-triggering directive's effect.
    fn ack(&self, state_version: u64) {
        self.outgoing.push(EngineFrame::AppliedStateVersion {
            v: nbe_protocol::PROTOCOL_VERSION.to_string(),
            state_version,
        });
    }

    fn schedule_done(&self, item_ref: String, duration_frames: u32, generation: u64) {
        let state = self.state.clone();
        let outgoing = self.outgoing.clone();
        let tracker = self.playing.clone();
        let rate = state.clock.lock().unwrap().house_rate();
        tokio::spawn(async move {
            let ms = (duration_frames as f64 / rate as f64 * 1000.0) as u64;
            sleep(Duration::from_millis(ms)).await;
            if !state.is_running() {
                return;
            }
            // A superseding take bumped the generation; do not emit a stale end.
            if !tracker.is_current(&item_ref, generation) {
                return;
            }
            outgoing.push(EngineFrame::ItemEvent {
                v: nbe_protocol::PROTOCOL_VERSION.to_string(),
                item_ref,
                event: ItemEvent::End,
                detail: None,
            });
        });
    }
}

/// The fallback asset must be resident (here: read into memory) after load.
fn load_fallback_asset(package_path: &str) -> Result<FallbackSlate, DirectiveError> {
    let manifest_path = std::path::Path::new(package_path).join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_reader(
        std::fs::File::open(&manifest_path)
            .map_err(|e| DirectiveError::Invalid(format!("cannot open manifest: {e}")))?,
    )
    .map_err(|e| DirectiveError::Invalid(format!("manifest not valid JSON: {e}")))?;
    let fallback_id = manifest
        .get("show")
        .and_then(|s| s.get("fallbackAssetId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| DirectiveError::Invalid("manifest has no fallbackAssetId".into()))?;
    let src = manifest
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|a| a.get("id").and_then(|v| v.as_str()) == Some(fallback_id))
        })
        .and_then(|a| a.get("source"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| DirectiveError::Invalid("fallback asset has no source".into()))?;
    let path = std::path::Path::new(package_path).join(src);
    let bytes = std::fs::read(&path)
        .map_err(|e| DirectiveError::FallbackMissing(format!("{}: {e}", path.display())))?;
    Ok(FallbackSlate { path, bytes })
}

fn duration_frames(d: &DirectiveFrame) -> Option<u32> {
    d.payload
        .get("durationFrames")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
}

/// `show.stop` payload flag with a SPEC default (both default when absent:
/// `quiesceOutputs: true`, `force: false`).
fn payload_bool(payload: &serde_json::Value, key: &str, default: bool) -> bool {
    payload
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

/// Derive the show filename component from the loaded package (SPEC: the
/// `record.start` directive carries no naming — `{ outputId? }` only).
/// Reads `<packagePath>/manifest.json` `show.title` (else `show.id`); any
/// failure (no package, unreadable manifest) falls back to `"show"` with a
/// warning — naming must never refuse a take. Read at start time (not cached
/// at load) so `show.load` stays untouched by recording concerns.
fn record_show_name(state: &SharedEngineState) -> String {
    let path = state.package_path.lock().unwrap().clone();
    let Some(path) = path else {
        return "show".into();
    };
    let manifest_path = std::path::Path::new(&path).join("manifest.json");
    let name = std::fs::read(&manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|m| {
            m.get("show").and_then(|s| {
                s.get("title")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| s.get("id").and_then(|v| v.as_str()).map(str::to_string))
            })
        })
        .filter(|s| !s.is_empty());
    match name {
        Some(n) => n,
        None => {
            tracing::warn!(
                path = %manifest_path.display(),
                "record.start: show name unreadable, falling back to \"show\""
            );
            "show".into()
        }
    }
}

/// WU2 — the record output's frame-path preference (`show.outputs.record`
/// `tapPath`, SPEC v0.4.5): `auto` (or an absent/unreadable field) defers to
/// the published selection table, `cpuReadback` restricts the take to CPU.
/// Read at start time (not cached at load) so `show.load` stays untouched by
/// recording concerns — the same shape as [`record_show_name`]. Returns the
/// override for [`crate::record::tap_path::select_with_override`]: `None`
/// means "no override, the table speaks".
fn record_tap_override(state: &SharedEngineState) -> Option<crate::record::tap_path::TapPath> {
    let path = state.package_path.lock().unwrap().clone()?;
    let bytes = std::fs::read(std::path::Path::new(&path).join("manifest.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let outputs: nbe_core::manifest::OutputDefaults =
        serde_json::from_value(manifest.get("show")?.get("outputs")?.clone()).ok()?;
    match outputs.record?.tap_path {
        nbe_core::manifest::TapPathPreference::CpuReadback => {
            Some(crate::record::tap_path::TapPath::CpuReadback)
        }
        nbe_core::manifest::TapPathPreference::Auto => None,
    }
}

/// WU4 — the loaded package's stream output (`show.outputs.stream`), read
/// once per `stream.start` (not cached at load, so `show.load` stays untouched
/// by streaming concerns — the [`record_show_name`] shape). `None` when no
/// package is loaded or it declares no stream output.
fn stream_manifest_output(state: &SharedEngineState) -> Option<nbe_core::manifest::StreamOutput> {
    let path = state.package_path.lock().unwrap().clone()?;
    let bytes = std::fs::read(std::path::Path::new(&path).join("manifest.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let outputs: nbe_core::manifest::OutputDefaults =
        serde_json::from_value(manifest.get("show")?.get("outputs")?.clone()).ok()?;
    outputs.stream
}

/// WU4 — the stream output's frame-path preference (`tapPath`, SPEC v0.4.5):
/// `auto` (or no stream output) defers to the published selection table,
/// `cpuReadback` restricts the stream to CPU — which `stream.start` refuses.
fn stream_tap_override(
    output: Option<&nbe_core::manifest::StreamOutput>,
) -> Option<crate::record::tap_path::TapPath> {
    match output?.tap_path {
        nbe_core::manifest::TapPathPreference::CpuReadback => {
            Some(crate::record::tap_path::TapPath::CpuReadback)
        }
        nbe_core::manifest::TapPathPreference::Auto => None,
    }
}

/// WU3 — `stream.start` url precedence (SPEC v0.4.5 §9.4, §16.14): the
/// command's `url` OVERRIDES `outputs.stream.url` for the run; the manifest
/// answers when the command is silent; NEITHER present is refused.
///
/// Empty-string rule (decided in WU3, pinned by
/// `tests/stream_url_precedence.rs`): a missing, empty, or whitespace-only
/// `url` counts as SILENT, never as an endpoint — an empty string must not
/// become a valid publish target. Returned urls are trimmed.
///
/// Scheme rule: the winner must be an `rtmp://` publish target
/// (case-insensitive match) — anything else is refused `E_BAD_PAYLOAD`-shaped
/// here, so garbage never goes Live with `publisher=None`. A non-string `url`
/// never reaches this resolver: `on_stream_start` refuses it first.
///
/// The refusal is `E_BAD_PAYLOAD`-shaped: the engine has no dedicated
/// `BadPayload` variant, so — like `marker.add`'s missing name and
/// `record.start`'s missing record directory — it surfaces as
/// [`DirectiveError::Invalid`], with the `E_BAD_PAYLOAD` token in the message.
///
/// Wired at [`DirectiveHandler::apply`]'s `"stream.start"` arm, which calls
/// `resolve_stream_url(manifest_url, command_url)` where `manifest_url` is
/// the loaded package's `show.outputs.stream.url` and `command_url` is the
/// command's string `url` (or `None` when silent/non-string-refused), and
/// propagates the `Err` (which withholds the ack, like every other failed
/// handler).
pub fn resolve_stream_url(
    manifest_url: Option<&str>,
    command_url: Option<&str>,
) -> Result<String, DirectiveError> {
    let present = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    let winner = if let Some(url) = command_url.and_then(present) {
        url
    } else if let Some(url) = manifest_url.and_then(present) {
        url
    } else {
        return Err(DirectiveError::Invalid(
            "E_BAD_PAYLOAD: stream.start needs a publish target: neither \
             outputs.stream.url nor the command's url supplied one"
                .into(),
        ));
    };
    if !winner.to_ascii_lowercase().starts_with("rtmp://") {
        return Err(DirectiveError::Invalid(format!(
            "E_BAD_PAYLOAD: stream.start url is not an rtmp:// publish target: {winner}"
        )));
    }
    Ok(winner)
}

/// Ceiling for the record path: 1920x1080. Refusing above it is deliberate —
/// the loop records what is on air via a View readback, and readback-alone
/// costs ≈48 ms at 4K, which exceeds any 30/60 fps frame budget before a
/// single sample is encoded. Recording above the ceiling would therefore shed
/// every frame by construction (the budget pre-check skips) while pretending
/// to record. Refused with engine-local `E_UNSUPPORTED`, loudly, before any
/// pipeline is reserved. (Step-0b answer-changer #3 made this law.)
pub const MAX_RECORD_WIDTH: u32 = 1920;
/// See [`MAX_RECORD_WIDTH`].
pub const MAX_RECORD_HEIGHT: u32 = 1080;

/// Enforce the record resolution ceiling ([`MAX_RECORD_WIDTH`]x[`MAX_RECORD_HEIGHT`]).
/// The View is exactly at-cap today, so this is a backstop against a future
/// geometry bump silently turning every take into shed frames — tested
/// directly, since the live path cannot currently exceed the ceiling.
pub fn check_record_resolution(width: u32, height: u32) -> Result<(), DirectiveError> {
    if width > MAX_RECORD_WIDTH || height > MAX_RECORD_HEIGHT {
        return Err(DirectiveError::Unsupported(format!(
            "record geometry {width}x{height} exceeds the {MAX_RECORD_WIDTH}x{MAX_RECORD_HEIGHT} ceiling; \
             4K readback-alone (~48ms) exceeds any 30/60fps budget"
        )));
    }
    Ok(())
}

/// Filename timestamp when the directive carries none: millisecond granularity
/// plus a per-process counter suffix, sanitizer-safe (the writer keeps
/// `[A-Za-z0-9._-]`). Second granularity truncated stop→restart takes in the
/// same second onto one filename (`File::create` truncates); this guarantees
/// distinct files for rapid takes.
fn default_timestamp() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("session-{millis}-{n}")
}

/// Writer/file failures underneath record stop/start keep their stable tokens
/// (`E_DISK`, `E_AAC_UNAVAILABLE`, `E_RECORD_INPUT`) — including `E_DISK`,
/// which has its own variant so the token survives instead of dissolving
/// into a bare io error.
fn finish_err(e: crate::record::RecordError) -> DirectiveError {
    match e {
        crate::record::RecordError::Disk(msg) => DirectiveError::Disk(msg),
        other => DirectiveError::Invalid(other.to_string()),
    }
}

/// Thread-take failures map the same way, plus the pipeline's own shapes:
/// no encoder (SPEC precondition, wired — no dead variant) and the bounded-
/// wait timeout (engine-local token, file kept, error surfaces).
fn session_err(e: crate::record::session::SessionError) -> DirectiveError {
    match e {
        crate::record::session::SessionError::NoEncoder(msg) => {
            DirectiveError::NoHardwareEncoder(msg)
        }
        crate::record::session::SessionError::Timeout(msg) => DirectiveError::Timeout(msg),
        crate::record::session::SessionError::Record(r) => finish_err(r),
    }
}

/// Stream session failures keep their stable tokens (`E_NO_ZEROCOPY`,
/// `E_NETWORK`) verbatim inside `Invalid` — the `E_BAD_PAYLOAD` convention
/// (no dedicated variant; the token in the message is the contract).
fn stream_err(e: crate::record::stream::StreamError) -> DirectiveError {
    DirectiveError::Invalid(e.to_string())
}
