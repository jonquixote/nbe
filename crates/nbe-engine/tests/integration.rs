//! Prompt 03 Step 7 integration: a real loopback WebSocket server speaking the
//! bridge protocol drives the engine process through connect-resync-directive.

use futures_util::{SinkExt, StreamExt};
use nbe_engine::channel::{self, EngineConfig};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, PROTOCOL_VERSION};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

/// In-process control plane: static snapshot-resync producer; asserts the
/// engine acknowledges and then applies subsequent directives.
async fn mock_control_plane(
    port_sender: tokio::sync::oneshot::Sender<(SocketAddr, tokio::task::JoinHandle<()>)>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (stream, _peer) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        // §5.9.4: resync first, before any directive.
        let resync_payload = serde_json::json!({
            "v": PROTOCOL_VERSION,
            "kind": "directive",
            "seq": 0,
            "stateVersion": 3,
            "command": "show.resync",
            "target": {},
            "payload": {
                "showState": "RUNNING",
                "viewItem": "A1",
                "previewItem": null,
                "itemStates": {"A1": "LIVE"},
                "sceneStates": {"SCN_A1": "VIEW"},
                "visibleOverlays": [],
                "automationHold": false,
                "fallbackActive": false,
                "stateVersion": 3
            }
        });
        let resync: DirectiveFrame = serde_json::from_value(resync_payload).unwrap();
        ws.send(Message::Text(
            serde_json::to_string(&resync).unwrap().into(),
        ))
        .await
        .unwrap();

        // Then the true "reconnect is over" boundary: the engine must ack.
        while let Some(Ok(msg)) = ws.next().await {
            if !msg.is_text() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            if v.get("kind").and_then(|k| k.as_str()) == Some("appliedStateVersion")
                && v.get("stateVersion").and_then(|x| x.as_u64()) == Some(3)
            {
                // The engine acked resync 3; now issue the take.
                let take: DirectiveFrame = serde_json::from_value(serde_json::json!({
                    "v": PROTOCOL_VERSION,
                    "kind": "directive",
                    "seq": 1,
                    "stateVersion": 4,
                    "command": "view.take",
                    "target": {"itemRef": "A1"},
                    "payload": {"transition": "cut"}
                }))
                .unwrap();
                ws.send(Message::Text(serde_json::to_string(&take).unwrap().into()))
                    .await
                    .unwrap();
            }
            // Read a telemetry frame too if present; we don't need to assert it
            // here — the engine's tick is what the integration test covers.
        }
    });
    port_sender.send((addr, handle)).unwrap();
}

#[tokio::test]
async fn engine_connects_resyncs_applies_take() {
    // The engine under test runs locally; the mock control plane drives it.
    let (sender, receiver) =
        tokio::sync::oneshot::channel::<(SocketAddr, tokio::task::JoinHandle<()>)>();
    tokio::spawn(mock_control_plane(sender));
    let (addr, server_handle) = receiver.await.unwrap();

    // Engine state: house_rate driven by config.
    let state: Arc<EngineState> = Arc::new(EngineState::new(30));
    let outgoing: Arc<OutgoingQueue> = Arc::new(Default::default());

    let cfg = EngineConfig {
        control_plane_url: format!("ws://{}/nbe/v0.3", addr),
        token: "render-token".into(), // mock server ignores headers here (its own auth knows best)
        house_rate: 30,
        telemetry_interval_ms: 250,
    };

    // Run the engine — it should connect, resync, ack, apply the take, and keep ticking.
    let engine_task = tokio::spawn(channel::run_forever(cfg, state.clone(), outgoing));

    // Let the interaction run a moment, then assert the state the mock expected.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if state.last_applied() >= 4 {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!(
                "engine never applied stateVersion 4 (take); last_applied={}",
                state.last_applied()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(state.is_running(), "resync must have started the clock");
    assert_eq!(state.clock_state(), nbe_engine::clock::ClockState::Running);
    // Outage loop: cut the task and assert its fallback is untouched and the
    // state is still there when the connection dies (local survivability).
    engine_task.abort();
    assert!(!state.fallback_active.load(Ordering::SeqCst));
    drop(server_handle);
}
