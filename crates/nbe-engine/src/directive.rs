//! Directive intake (Prompt 03 Step 3/3a/4a): apply control-plane directives
//! to the engine's clock/state, verify fallback residency at show.load, and
//! acknowledge independently of the command path.

use crate::state::{FallbackSlate, SharedEngineState, SharedOutgoing};
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
            "show.load" => self.on_show_load(d)?,
            "show.start" => self.on_show_start(d)?,
            "show.stop" => self.on_show_stop(d)?,
            "view.take" | "view.cut" => self.on_take(d)?,
            "view.fallback" => self.on_fallback(d)?,
            "soundboard.play" | "soundboard.stop" | "soundboard.stopAll" | "audio.bus.set"
            | "audio.duck" | "guest.mute" => self.on_audio(d)?,
            nbe_protocol::command::RESYNC => self.on_resync(d)?,
            other => {
                debug!(command = other, "directive ignored (no engine effect)");
            }
        }
        // Ack every applied directive (SPEC 5.9.3: appliedStateVersion is the
        // engine's most recent applied stateVersion — the honest signal for
        // /status and the show.stop grace window). One emission point, called
        // exactly once per applied directive.
        self.state.set_last_applied(d.state_version);
        self.ack(d.state_version);
        Ok(())
    }

    fn on_show_load(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
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

        // Prompt 05: decode the package's video assets here, at load time.
        // A genuine decode failure IS a fault — unlike Prompt 04's scope
        // boundary — and is reported as `itemEvent: decodeError` so the
        // control plane can drive the item to ERROR (SPEC §5.9.3, §17.3).
        let budget = crate::loop_cache::CacheBudget {
            per_loop_mib: 1024,
            total_mib: 4096,
            recommended_working_set_mib: None,
        };
        let mut library = crate::video::VideoLibrary::default();
        for (asset_id, kind) in &index.asset_kind {
            if kind != "video" && kind != "alphaVideo" {
                continue;
            }
            let Some(src) = index.asset_source.get(asset_id) else {
                continue;
            };
            let declared = index.declared_loop_period.get(asset_id).copied();
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

                    // `itemEvent` is addressed to a rundown Item, because that
                    // is what the §17.3 state machine tracks. Reporting an
                    // asset id here would produce a fault the control plane
                    // cannot attribute to anything.
                    let affected = index.items_using_asset(asset_id);
                    if affected.is_empty() {
                        tracing::warn!(
                            asset = %asset_id,
                            "decode failure affects no rundown item; nothing to report"
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
    fn on_show_stop(&self, _d: &DirectiveFrame) -> Result<(), DirectiveError> {
        self.state.clock.lock().unwrap().stop();
        // Outputs are stubs in this prompt; the protocol shape is the point.
        Ok(())
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
            // SPEC §12.1: the incoming item's timeline starts at the frame it
            // goes on air, and the outgoing item keeps reading from its own
            // start for the length of a mix.
            let previous_start = self
                .state
                .view_item_start_frame
                .load(std::sync::atomic::Ordering::SeqCst);
            *self.state.transition.lock().unwrap() = Some(crate::scene::Transition {
                from_item: previous,
                from_start_frame: previous_start,
                to_item: r.to_string(),
                kind,
                duration_frames: transition_frames,
                start_frame,
            });
            *self.state.view_item.lock().unwrap() = Some(r.to_string());
            self.state
                .view_item_start_frame
                .store(start_frame, std::sync::atomic::Ordering::SeqCst);
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

    fn on_fallback(&self, _d: &DirectiveFrame) -> Result<(), DirectiveError> {
        self.state.fallback_active.store(true, Ordering::SeqCst);
        *self.state.view_item.lock().unwrap() = None; // on fallback, view shows the slate
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
            // SPEC §5.9.4's snapshot names WHAT is on air but not since when,
            // so a resynced timed item resumes from its first frame rather
            // than guessing an origin. Recorded as a spec gap in
            // agents/prompts/05-video-decode.md.
            self.state
                .view_item_start_frame
                .store(now, std::sync::atomic::Ordering::SeqCst);
            // A resync supersedes any transition the engine was mid-way
            // through: the snapshot is the state, not a waypoint toward it.
            *self.state.transition.lock().unwrap() = None;
        }
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
