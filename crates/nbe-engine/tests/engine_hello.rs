//! Review4 step 7a (the user's ruling of 2026-11-16 23:45): the engine says
//! what it holds, first on every connection, so a control plane can tell a
//! restarted engine from a reconnect (review4 case E) and a restarted control
//! plane can adopt a running show (step 7b).

use futures_util::StreamExt;
use nbe_engine::channel::{self, EngineConfig};
use nbe_engine::state::{EngineState, LoadedPackage, OutgoingQueue, OverlayPhase, OverlayRuntime};
use nbe_protocol::{EngineFrame, HelloPackage};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

fn fields(frame: EngineFrame) -> nbe_protocol::EngineHello {
    match frame {
        EngineFrame::EngineHello { fields, .. } => *fields,
        other => panic!("expected engineHello, got {other:?}"),
    }
}

#[test]
fn a_fresh_engine_says_it_holds_nothing_with_nulls() {
    let state = EngineState::new(30);
    let h = fields(channel::hello(&state));
    assert_eq!(h.engine_instance_id, state.instance_id);
    assert!(!h.show_running);
    assert_eq!(
        h.master_frame, None,
        "a clock that never started has no frame"
    );
    assert_eq!(h.last_applied_state_version, 0);
    assert_eq!(h.package, None);
    assert_eq!(
        (h.view_item, h.view_item_start_frame, h.preview_item),
        (None, None, None)
    );
    assert!(h.visible_overlays.is_empty());
    assert!(!h.fallback_active);
    assert!(!h.recording);
    assert_eq!(h.stream_transport_state, "none", "never streamed");
}

#[test]
fn the_hello_reports_what_the_engine_holds() {
    let state = EngineState::new(30);
    state.clock.lock().unwrap().start();
    state.set_last_applied(41);
    *state.loaded_package.lock().unwrap() = Some(LoadedPackage {
        path: "/shows/evening".into(),
        load_state_version: 12,
    });
    *state.view_item.lock().unwrap() = Some("A1".into());
    state
        .view_item_start_frame
        .store(7, std::sync::atomic::Ordering::SeqCst);
    *state.preview_item.lock().unwrap() = Some("A2".into());
    {
        let mut overlays = state.overlays.lock().unwrap();
        for (id, on_air) in [("ticker", true), ("lower_third", true), ("gone", false)] {
            overlays.insert(
                id.into(),
                OverlayRuntime {
                    on_air,
                    anim_start: 0,
                    duration_frames: 1,
                    phase: OverlayPhase::Steady,
                },
            );
        }
    }
    let h = fields(channel::hello(&state));
    assert!(h.show_running);
    assert!(h.master_frame.is_some());
    assert_eq!(h.last_applied_state_version, 41);
    assert_eq!(
        h.package,
        Some(HelloPackage {
            path: "/shows/evening".into(),
            load_state_version: 12
        })
    );
    assert_eq!(h.view_item.as_deref(), Some("A1"));
    assert_eq!(h.view_item_start_frame, Some(7));
    assert_eq!(h.preview_item.as_deref(), Some("A2"));
    assert_eq!(
        h.visible_overlays,
        vec!["lower_third".to_string(), "ticker".into()],
        "on air only, sorted"
    );
}

#[test]
fn an_instance_id_is_fixed_for_one_engine_and_differs_between_two() {
    let one = EngineState::new(30);
    let a = fields(channel::hello(&one)).engine_instance_id;
    let b = fields(channel::hello(&one)).engine_instance_id;
    assert_eq!(a, b, "the same process, the same id");
    let two = EngineState::new(30);
    assert_ne!(a, two.instance_id, "a new engine, a new id");
}

/// The hello is the FIRST frame on every connection, ahead of the telemetry
/// tick (which the pump sends at once), and a reconnect carries the same id.
#[tokio::test]
async fn the_hello_is_first_on_every_connection_with_one_id_across_reconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let firsts = tokio::spawn(async move {
        let mut out = Vec::new();
        for _ in 0..3 {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(sock).await.unwrap();
            let first = loop {
                match ws.next().await {
                    Some(Ok(m)) if m.is_text() => break m.into_text().unwrap().to_string(),
                    Some(Ok(_)) => continue,
                    other => panic!("no first frame: {other:?}"),
                }
            };
            out.push(first);
            drop(ws); // the engine reconnects
        }
        out
    });
    let state = Arc::new(EngineState::new(30));
    let cfg = EngineConfig {
        control_plane_url: format!("ws://{addr}/nbe/v0.3"),
        token: "render-token".into(),
        house_rate: 30,
        telemetry_interval_ms: 1,
    };
    let engine = tokio::spawn(channel::run_forever(
        cfg,
        state.clone(),
        Arc::new(OutgoingQueue::default()),
    ));
    let firsts = tokio::time::timeout(Duration::from_secs(20), firsts)
        .await
        .expect("three connections within 20 s")
        .unwrap();
    engine.abort();
    for (i, text) in firsts.iter().enumerate() {
        let frame: EngineFrame =
            serde_json::from_str(text).unwrap_or_else(|e| panic!("connection {i}: {e}: {text}"));
        assert_eq!(
            fields(frame).engine_instance_id,
            state.instance_id,
            "connection {i}"
        );
    }
}
