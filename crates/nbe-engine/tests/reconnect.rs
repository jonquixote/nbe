//! Prompt 03 Step 7 integration + reconnect repro: engine connects, resyncs,
//! applies, loses the connection, reconnects and STILL applies (the 03b
//! reconnect-deafness repro). A test that used to hang green now must pass.

use futures_util::SinkExt;
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

/// A control-plane double that fully services connection 1, drops it, then
/// services connection 2 with a fresh resync followed by a take.
async fn mock_two_connection_server(
    ready: tokio::sync::oneshot::Sender<SocketAddr>,
) -> JoinHandle<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(async move {
        // Connection 1: resync, send take, close.
        let (sock, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(sock).await.unwrap();
        let r = serde_json::from_value::<DirectiveFrame>(make_resync(1, 0)).unwrap();
        ws.send(Message::Text(serde_json::to_string(&r).unwrap().into()))
            .await
            .unwrap();
        let t1 = serde_json::from_value::<DirectiveFrame>(make_take(1, 2)).unwrap();
        ws.send(Message::Text(serde_json::to_string(&t1).unwrap().into()))
            .await
            .unwrap();
        // drop the socket
        drop(ws);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connection 2: resync, then take (seq 0, stateVersion 3).
        let (sock2, _) = listener.accept().await.unwrap();
        let mut ws2 = accept_async(sock2).await.unwrap();
        let r2 = serde_json::from_value::<DirectiveFrame>(make_resync(3, 0)).unwrap();
        ws2.send(Message::Text(serde_json::to_string(&r2).unwrap().into()))
            .await
            .unwrap();
        // A moment to allow the engine to ack and settle on the new conn.
        tokio::time::sleep(Duration::from_millis(40)).await;
        let t2 = serde_json::from_value::<DirectiveFrame>(make_take(1, 4)).unwrap();
        ws2.send(Message::Text(serde_json::to_string(&t2).unwrap().into()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(ws2);
    });

    ready.send(addr).unwrap();
    handle
}

#[tokio::test]
async fn engine_applies_directives_after_reconnect() {
    let (tx, rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let server_handle = mock_two_connection_server(tx).await;
    let addr = rx.await.unwrap();

    let state: Arc<EngineState> = Arc::new(EngineState::new(30));
    let outgoing: Arc<OutgoingQueue> = Arc::new(Default::default());
    let cfg = EngineConfig {
        control_plane_url: format!("ws://{}/nbe/v0.3", addr),
        token: "render-token".into(),
        house_rate: 30,
        telemetry_interval_ms: 10,
    };
    let st_clone = state.clone();
    let eng = tokio::spawn(channel::run_forever(cfg, st_clone, outgoing));

    // The second connection's take must land — the reconnect-deafness
    // regression fires when this assert times out at sv=2 forever.
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
    eng.abort();
    server_handle.abort();
}

#[tokio::test]
async fn engine_ack_includes_every_applied_directive() {
    // Covers the Step 3 ack contract: after two applied directives, the
    // pump's outgoing queue carries two acks.
    let (tx, rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let listener_handle = mock_two_connection_server(tx).await;
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
    let out = outgoing.clone();
    let engine = tokio::spawn(channel::run_forever(cfg, st, out));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while state.last_applied() < 4 {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let frames = outgoing.drain();
    let acks: Vec<u64> = frames
        .into_iter()
        .filter_map(|f| match f {
            nbe_protocol::EngineFrame::AppliedStateVersion { state_version, .. } => {
                Some(state_version)
            }
            _ => None,
        })
        .collect();
    assert!(
        !acks.is_empty(),
        "engine must ack at least the applied directives"
    );
    engine.abort();
    listener_handle.abort();
}
