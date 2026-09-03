//! WebSocket channel to the control plane (Step 2/2a): connect with the
//! render-role handshake, reconnect with backoff, gate directive application
//! behind show.resync, pump outbound engine frames from a shared queue, and
//! never let channel I/O sit on the same task as the master clock or telemetry.

use crate::directive::DirectiveHandler;
use crate::state::{SharedEngineState, SharedOutgoing};
use crate::telemetry::build_tick;
use futures_util::{SinkExt, StreamExt};
use nbe_protocol::DirectiveFrame;
use std::time::Duration;
use tokio::sync::mpsc;

use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

pub struct EngineConfig {
    pub control_plane_url: String,
    pub token: String,
    pub house_rate: u32,
    /// outbound pump cadence in ms (1 Hz telemetry target).
    pub telemetry_interval_ms: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            control_plane_url: "ws://127.0.0.1:8462/nbe/v0.3".to_string(),
            token: String::new(),
            house_rate: 30,
            telemetry_interval_ms: 1000,
        }
    }
}

/// The engine-side channel master. Owns the connection and the four tasks
/// that share it.
pub struct RenderChannel {
    cfg: EngineConfig,
    state: SharedEngineState,
    outgoing: SharedOutgoing,
}

impl RenderChannel {
    pub fn new(cfg: EngineConfig, state: SharedEngineState, outgoing: SharedOutgoing) -> Self {
        Self {
            state,
            outgoing,
            cfg,
        }
    }

    /// Connect and process until the socket dies.
    pub async fn run(self) -> anyhow::Result<()> {
        info!(url = %self.cfg.control_plane_url, "connecting to control plane");
        let mut req = self.cfg.control_plane_url.as_str().into_client_request()?;
        req.headers_mut().insert(
            "authorization",
            format!("Bearer {}", self.cfg.token).parse()?,
        );
        req.headers_mut().insert("x-nbe-role", "render".parse()?);
        let (ws, _resp) = connect_async(req).await?;
        let (wss, wsr) = ws.split();

        // Each connection: pump engine frames out; read directives in; keep
        // telemetry separate from both via the queue.
        let (tx, mut rx) = mpsc::channel::<String>(256);
        let state = self.state.clone();
        let outgoing = self.outgoing.clone();
        let interval = Duration::from_millis(self.cfg.telemetry_interval_ms);

        // Task 1: outbound engine frames + telemetry tick.
        let out_tx = tx.clone();
        tokio::spawn(async move {
            loop {
                for frame in outgoing.drain() {
                    if let Ok(s) = serde_json::to_string(&frame) {
                        let _ = out_tx.send(s).await;
                    }
                }
                let tick = build_tick(&state);
                if let Ok(s) = serde_json::to_string(&tick) {
                    let _ = out_tx.send(s).await;
                }
                tokio::time::sleep(interval).await;
            }
        });

        // Task 2: send loop onto the WS (async).
        let mut wss = wss;
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if wss.send(Message::Text(msg.into())).await.is_err() {
                    error!("ws send failed");
                    break;
                }
            }
        });

        // Task 3 (this task): inbound directive processing. Never do anything
        // synchronized from here — only hand work to the handler.
        let state = self.state.clone();
        let outgoing = self.outgoing.clone();
        let handler = DirectiveHandler::new(state.clone(), self.outgoing.clone());
        let mut reader = wsr;
        let mut resynced = false;

        while let Some(item) = reader.next().await {
            let msg = match item {
                Ok(m) => m,
                Err(e) => {
                    warn!(err = %e, "control plane read failed");
                    break;
                }
            };
            if !msg.is_text() {
                continue;
            }
            let text = match msg.to_text() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let frame: DirectiveFrame = match serde_json::from_str(text) {
                Ok(f) => f,
                Err(e) => {
                    warn!(err = %e, "unparseable frame from control plane");
                    continue;
                }
            };
            if frame.command == nbe_protocol::command::RESYNC {
                handler.apply(&frame).await?;
                resynced = true;
                continue;
            }
            if !resynced {
                warn!(command = %frame.command, "directive before resync; dropping");
                continue;
            }
            // Out-of-order stateVersion: skip and log (Step 3).
            let applied = self.state.last_applied();
            if frame.state_version <= applied {
                warn!(
                    sv = frame.state_version,
                    applied, "stale stateVersion; skipping"
                );
                continue;
            }
            // seq gap = directives dropped on the wire; never infer — resync.
            let prev_seq = self
                .state
                .last_seq
                .load(std::sync::atomic::Ordering::SeqCst);
            if frame.seq != 0 && frame.seq != prev_seq + 1 {
                warn!(
                    have = prev_seq,
                    got = frame.seq,
                    "seq gap; requesting resync"
                );
                outgoing.push(nbe_protocol::EngineFrame::ResyncRequest {
                    v: nbe_protocol::PROTOCOL_VERSION.to_string(),
                    reason: nbe_protocol::ResyncReason::SeqGap,
                });
                continue;
            }
            self.state.note_seq(frame.seq);
            if let Err(e) = handler.apply(&frame).await {
                // A directive fails only when what it names cannot be executed.
                // Log and carry on; engine keeps last known state.
                warn!(err = %e, command = %frame.command, "directive failed");
            }
        }
        Err(anyhow::anyhow!("control plane closed"))
    }
}

/// Entry point with reconnect/backoff. A control-plane outage MUST NOT stop
/// the engine (SPEC 9.5-style survivability).
pub async fn run_forever(cfg: EngineConfig, state: SharedEngineState, outgoing: SharedOutgoing) {
    let mut delay_ms = 100u64;
    loop {
        let ch = RenderChannel::new(cfg_clone(&cfg), state.clone(), outgoing.clone());
        match ch.run().await {
            Ok(()) => {
                delay_ms = 100;
            }
            Err(e) => {
                warn!(err = %e, delay_ms, "control plane connection dropped; reconnecting");
            }
        }
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms = (delay_ms * 2).min(10_000);
    }
}

fn cfg_clone(c: &EngineConfig) -> EngineConfig {
    EngineConfig {
        control_plane_url: c.control_plane_url.clone(),
        token: c.token.clone(),
        house_rate: c.house_rate,
        telemetry_interval_ms: c.telemetry_interval_ms,
    }
}
