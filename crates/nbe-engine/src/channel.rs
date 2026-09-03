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

/// Per-connection state. The gate: directives are dropped until `show.resync`
/// arrives (Step 2a). The `expected_seq` is per-connection — the control
/// plane resets its `seq` on each reconnect (§5.9.2), so a connection-local
/// expectation is the only meaningful one. A prior connection's seq must
/// never reach into the next one, or the second connection spends its life
/// in spurious gaps.
pub(crate) struct ConnectionGate {
    resynced: bool,
    expected_seq: Option<u64>,
}

/// What the gate decided about a frame. Testable in isolation (Step 4 covers
/// all four outcomes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateDecision {
    Accept,
    WaitForResync,
    StaleStateVersion,
    SeqGap,
}

impl ConnectionGate {
    fn new() -> Self {
        Self {
            resynced: false,
            expected_seq: None,
        }
    }

    fn on_resync(&mut self, frame: &DirectiveFrame) {
        self.resynced = true;
        self.expected_seq = Some(frame.seq + 1);
    }

    fn classify(&mut self, frame: &DirectiveFrame, applied_state_version: u64) -> GateDecision {
        if !self.resynced {
            return GateDecision::WaitForResync;
        }
        if frame.state_version <= applied_state_version {
            return GateDecision::StaleStateVersion;
        }
        if let Some(expect) = self.expected_seq {
            if frame.seq != expect {
                return GateDecision::SeqGap;
            }
            self.expected_seq = Some(expect + 1);
        }
        GateDecision::Accept
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

        // Task 1: outbound engine frames + telemetry tick. Bound to the
        // connection's lifetime — pump_tasks.abort() on drop-out (Step 2).
        let out_tx = tx.clone();
        let pump_state = state.clone();
        let pump_outgoing = outgoing.clone();
        let pump = tokio::spawn(async move {
            loop {
                for frame in pump_outgoing.drain() {
                    if let Ok(s) = serde_json::to_string(&frame) {
                        let _ = out_tx.send(s).await;
                    }
                }
                let tick = build_tick(&pump_state);
                if let Ok(s) = serde_json::to_string(&tick) {
                    let _ = out_tx.send(s).await;
                }
                tokio::time::sleep(interval).await;
            }
        });

        // Task 2: send loop onto the WS.
        let mut wss = wss;
        let sender = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if wss.send(Message::Text(msg.into())).await.is_err() {
                    error!("ws send failed");
                    break;
                }
            }
        });

        // Task 3 (this task): inbound directive processing. Never do anything
        // synchronized from here — only hand work to the handler.
        let handler = DirectiveHandler::new(state.clone(), self.outgoing.clone());
        let mut reader = wsr;
        let mut gate = ConnectionGate::new();

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
                gate.on_resync(&frame);
                handler.apply(&frame).await?;
                continue;
            }
            let decision = gate.classify(&frame, state.last_applied());
            match decision {
                GateDecision::WaitForResync => {
                    warn!(command = %frame.command, "directive before resync; dropping");
                    continue;
                }
                GateDecision::StaleStateVersion => {
                    warn!(sv = frame.state_version, "stale stateVersion; skipping");
                    continue;
                }
                GateDecision::SeqGap => {
                    warn!(
                        seq = frame.seq,
                        "seq gap; requesting resync (never infer from a drop)"
                    );
                    self.outgoing
                        .push(nbe_protocol::EngineFrame::ResyncRequest {
                            v: nbe_protocol::PROTOCOL_VERSION.to_string(),
                            reason: nbe_protocol::ResyncReason::SeqGap,
                        });
                    continue;
                }
                GateDecision::Accept => { /* fall through to apply */ }
            }
            let _ = &frame;
            if let Err(e) = handler.apply(&frame).await {
                warn!(err = %e, command = %frame.command, "directive failed");
            }
        }
        // The two spawned tasks must not outlive the connection (Step 2).
        pump.abort();
        sender.abort();
        Err(anyhow::anyhow!("control plane closed"))
    }
}

/// Entry point with reconnect/backoff. A control-plane outage MUST NOT stop
/// the engine (SPEC 9.5-style survivability).
pub async fn run_forever(cfg: EngineConfig, state: SharedEngineState, outgoing: SharedOutgoing) {
    let mut delay_ms = 100u64;
    loop {
        let ch = RenderChannel::new(cfg_clone(&cfg), state.clone(), outgoing.clone());
        let connect_started = std::time::Instant::now();
        match ch.run().await {
            Ok(()) => {
                warn!("control plane connection ended cleanly; reconnecting");
                delay_ms = 100;
            }
            Err(e) => {
                // A connection that survived at least one full backoff window
                // before failing is "a successful connect" — reset the backoff.
                let connected_ms = connect_started.elapsed().as_millis() as u64;
                if connected_ms >= delay_ms {
                    delay_ms = 100;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use nbe_protocol::{command, DirectiveKind, PROTOCOL_VERSION};

    fn mk(command: &str, seq: u64, sv: u64) -> DirectiveFrame {
        DirectiveFrame {
            v: PROTOCOL_VERSION.into(),
            kind: DirectiveKind::Directive,
            seq,
            state_version: sv,
            command: command.into(),
            target: serde_json::json!({}),
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn gate_blocks_all_until_resync() {
        let mut g = ConnectionGate::new();
        assert_eq!(
            g.classify(&mk("view.take", 0, 1), 0),
            GateDecision::WaitForResync
        );
    }

    #[test]
    fn gate_accepts_after_resync_and_tracks_seq() {
        let mut g = ConnectionGate::new();
        g.on_resync(&mk(command::RESYNC, 0, 10));
        assert_eq!(
            g.classify(&mk("show.start", 1, 11), 10),
            GateDecision::Accept
        );
        assert_eq!(
            g.classify(&mk("show.stop", 2, 12), 11),
            GateDecision::Accept
        );
    }

    #[test]
    fn gate_rejects_stale_stateversion() {
        let mut g = ConnectionGate::new();
        g.on_resync(&mk(command::RESYNC, 0, 10));
        assert_eq!(
            g.classify(&mk("view.take", 1, 5), 10),
            GateDecision::StaleStateVersion
        );
    }

    #[test]
    fn gate_detects_seq_gap_and_never_guesses() {
        let mut g = ConnectionGate::new();
        g.on_resync(&mk(command::RESYNC, 0, 10));
        // expected seq 1; got 3
        assert_eq!(
            g.classify(&mk("show.stop", 3, 11), 10),
            GateDecision::SeqGap
        );
    }

    #[test]
    fn a_fresh_gate_forgets_prior_connection() {
        let mut g1 = ConnectionGate::new();
        g1.on_resync(&mk(command::RESYNC, 0, 20));
        g1.classify(&mk("show.start", 1, 21), 20);
        // New connection: seq resets to 0, resync resets expectations.
        let mut g2 = ConnectionGate::new();
        assert_eq!(
            g1.classify(&mk("view.take", 2, 22), 20),
            GateDecision::Accept
        );
        // Late directive from the second connection must wait for its resync.
        assert_eq!(
            g2.classify(&mk("view.take", 5, 30), 20),
            GateDecision::WaitForResync
        );
    }
}
