//! Prompt 03 Step 7 integration + 03b reconnect repro + 03c rewritten
//! per-directive ack contract test (reads frames off the wire, not the queue).

use futures_util::{SinkExt, StreamExt};
use nbe_engine::channel::{self, EngineConfig};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{command::RESYNC, DirectiveFrame, PROTOCOL_VERSION};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

fn make_take(seq: u64, state_version: u64) -> serde_json::Value {
    serde_json::json!({
        "v": PROTOCOL_VERSION,
        "kind": "directive",
        "seq": seq,
        "stateVersion": state_version,
        "command": "view.take",
        "target": {"itemRef": "A1"},
        "payload": {"transition": "cut"}
    })
}

fn make_resync(state_version: u64, seq: u64) -> serde_json::Value {
    serde_json::json!({
        "v": PROTOCOL_VERSION,
        "kind": "directive",
        "seq": seq,
        "stateVersion": state_version,
        "command": RESYNC,
        "target": {},
        "payload": {
            "showState": "RUNNING",
            "viewItem": "A1",
            "previewItem": null,
            "itemStates": {"A1": "LIVE"},
            "sceneStates": {},
            "visibleOverlays": [],
            "automationHold": false,
            "fallbackActive": false,
            "stateVersion": state_version
        }
    })
}

/// Wire-side recorder: the engine's client goes through this mock, which
/// captures incoming appliedStateVersion frames as they arrive.
struct MockServer {
    acks: Arc<std::sync::Mutex<Vec<u64>>>,
}

impl MockServer {
    fn new() -> Self {
        Self {
            acks: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    async fn run(&self, ready: tokio::sync::oneshot::Sender<SocketAddr>) -> JoinHandle<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acks = self.acks.clone();
        let handle = tokio::spawn(async move {
            for connection in 0..2u32 {
                let (sock, _) = listener.accept().await.unwrap();
                let mut ws = accept_async(sock).await.unwrap();
                let resync: DirectiveFrame =
                    serde_json::from_value(make_resync(1 + connection as u64 * 2, 0)).unwrap();
                ws.send(Message::Text(
                    serde_json::to_string(&resync).unwrap().into(),
                ))
                .await
                .unwrap();
                // Small grace so the engine's ack drains.
                tokio::time::sleep(Duration::from_millis(30)).await;
                // Send a take so the engine can ack it too.
                let take: DirectiveFrame =
                    serde_json::from_value(make_take(1, 2 + connection as u64 * 2)).unwrap();
                ws.send(Message::Text(serde_json::to_string(&take).unwrap().into()))
                    .await
                    .unwrap();

                // Collect any wire frames from the engine into acks.
                let deadline = tokio::time::Instant::now() + Duration::from_millis(400);
                let local_acks = acks.clone();
                while tokio::time::Instant::now() < deadline {
                    match tokio::time::timeout(Duration::from_millis(30), ws.next()).await {
                        Ok(Some(Ok(msg))) if msg.is_text() => {
                            if let Ok(v) =
                                serde_json::from_str::<serde_json::Value>(msg.to_text().unwrap())
                            {
                                if v.get("kind").and_then(|k| k.as_str())
                                    == Some("appliedStateVersion")
                                {
                                    if let Some(sv) = v.get("stateVersion").and_then(|x| x.as_u64())
                                    {
                                        local_acks.lock().unwrap().push(sv);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                drop(ws);
            }
        });
        ready.send(addr).unwrap();
        handle
    }
}

#[tokio::test]
async fn engine_applies_directives_after_reconnect() {
    let mock = MockServer::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let server_handle = mock.run(tx).await;
    let addr = rx.await.unwrap();

    let state: Arc<EngineState> = Arc::new(EngineState::new(30));
    let outgoing: Arc<OutgoingQueue> = Arc::new(Default::default());
    let cfg = EngineConfig {
        control_plane_url: format!("ws://{}/nbe/v0.3", addr),
        token: "render-token".into(),
        house_rate: 30,
        telemetry_interval_ms: 10,
    };
    let st = state.clone();
    let engine = tokio::spawn(channel::run_forever(cfg, st, outgoing.clone()));

    // Wait for the engine to apply the server's second take (sv 4).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while state.last_applied() < 4 {
        if tokio::time::Instant::now() > deadline {
            panic!(
                "engine never applied the post-reconnect directive; last_applied={}",
                state.last_applied()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(state.last_applied(), 4);
    engine.abort();
    server_handle.abort();
}

/// 03c Step 3: read acks off the wire — not the shared queue. Per-directive
/// acking means ack [1, 3] for the resyncs and [2, 4] for the takes, in order.
#[tokio::test]
async fn every_applied_directive_produces_an_ack_on_the_wire() {
    let mock = MockServer::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let server_handle = mock.run(tx).await;
    let addr = rx.await.unwrap();

    let state = Arc::new(EngineState::new(30));
    let outgoing: Arc<OutgoingQueue> = Arc::new(Default::default());
    let cfg = EngineConfig {
        control_plane_url: format!("ws://{}/nbe/v0.3", addr),
        token: "render-token".into(),
        house_rate: 30,
        telemetry_interval_ms: 10,
    };
    let st = state.clone();
    let engine = tokio::spawn(channel::run_forever(cfg, st, outgoing));

    // Wait long enough for both connections to complete their ack collection.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let acks = mock.acks.lock().unwrap().clone();
    assert_eq!(acks.len(), 4, "expected exactly 4 acks, got {acks:?}");
    assert_eq!(acks, vec![1, 2, 3, 4], "acks must be in apply order");
    engine.abort();
    server_handle.abort();
}
