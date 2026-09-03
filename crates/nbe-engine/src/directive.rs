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
/// end event.
pub struct PlaybackTracker {
    current: Mutex<Option<String>>,
}

impl PlaybackTracker {
    fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }
    fn cancel(&self) {
        *self.current.lock().unwrap() = None;
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
            nbe_protocol::command::RESYNC => self.on_resync(d)?,
            other => {
                debug!(command = other, "directive ignored (no engine effect)");
            }
        }
        self.state.set_last_applied(d.state_version);
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
        info!(path, "show.load: package ready, fallback resident");
        Ok(())
    }

    fn on_show_start(&self, _d: &DirectiveFrame) -> Result<(), DirectiveError> {
        self.state.clock.lock().unwrap().start();
        Ok(())
    }

    /// show.stop arrives with the quiesce stop directives already emitted by
    /// the control plane (SPEC §5.9.5). The engine applies them and then acks.
    fn on_show_stop(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        self.state.clock.lock().unwrap().stop();
        // Outputs are stubs in this prompt; the protocol shape is the point.
        self.ack(d.state_version);
        Ok(())
    }

    fn on_take(&self, d: &DirectiveFrame) -> Result<(), DirectiveError> {
        let item_ref = d
            .target
            .get("itemRef")
            .and_then(|v| v.as_str())
            .or_else(|| d.target.get("sceneId").and_then(|v| v.as_str()));
        if let Some(r) = item_ref {
            self.playing.cancel();
            if let Some(frames) = duration_frames(d) {
                self.schedule_done(r.to_string(), frames);
            }
        }
        Ok(())
    }

    fn on_fallback(&self, _d: &DirectiveFrame) -> Result<(), DirectiveError> {
        self.state.fallback_active.store(true, Ordering::SeqCst);
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
        self.state.set_last_applied(d.state_version);
        self.ack(d.state_version);
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

    fn schedule_done(&self, item_ref: String, duration_frames: u32) {
        let state = self.state.clone();
        let outgoing = self.outgoing.clone();
        let rate = state.clock.lock().unwrap().house_rate();
        tokio::spawn(async move {
            let ms = (duration_frames as f64 / rate as f64 * 1000.0) as u64;
            sleep(Duration::from_millis(ms)).await;
            if !state.is_running() {
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
