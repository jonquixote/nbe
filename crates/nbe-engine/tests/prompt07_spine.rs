//! Prompt 07, steps 1-2: the spine beneath the overlay level.
//!
//! R5 (the clock does not start promptly) and R6 (`appliedStateVersion` freezes
//! after resync) were reported as two findings. They are one defect each, and
//! neither is where the rehearsal's symptom pointed:
//!
//! * **R5 is not a clock defect.** `MasterClock` is `(now - epoch) * rate` and
//!   advances the instant `start()` is called. The clock reads 0 after
//!   `show.start` because `show.start` has not been *applied* yet: the inbound
//!   directive loop calls `handler.apply().await` inline, `show.load` decodes
//!   the whole package inside that call, and the decode is blocking CPU work on
//!   an async task. It head-of-line blocks every later directive AND starves
//!   the telemetry pump on the same runtime.
//!
//! * **R6 is not a freeze.** The engine acks every applied directive at one
//!   emission point in `DirectiveHandler::apply`. The acks sit in
//!   `OutgoingQueue` until the pump drains it — and the pump drains once per
//!   `telemetry_interval_ms`, so acknowledgements are quantised to 1 Hz. A
//!   `stateChange` frame snapshots `renderNode` at command-accept time, which is
//!   always before the ack can arrive, so every one of them reads the previous
//!   value.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION};

fn media(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/media")
        .join(name)
}

fn directive(command: &str, sv: u64, payload: serde_json::Value) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target: serde_json::json!({}),
        payload,
    }
}

/// A package with enough real video that decoding it is measurable work.
fn write_package(dir: &Path, clips: usize) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();

    let mut assets = vec![serde_json::json!(
        { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" }
    )];
    for i in 0..clips {
        std::fs::copy(media("cfr_30.mp4"), dir.join(format!("media/c{i}.mp4"))).unwrap();
        assets.push(serde_json::json!({
            "id": format!("c{i}"), "kind": "video",
            "source": format!("media/c{i}.mp4"), "format": "h264"
        }));
    }
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.4",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": assets,
            "scenes": [{ "id": "SCN", "elements": [
                { "id": "main", "kind": "clip", "z": 1, "assetId": "c0" }
            ]}],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// R5 — the clock starts promptly, because load does not own the runtime
// ---------------------------------------------------------------------------

/// SPEC §11 / rehearsal step 3. A single-threaded runtime, so "did another task
/// get to run" is a question with a yes/no answer rather than a race.
#[tokio::test(flavor = "current_thread")]
async fn a_package_load_does_not_starve_the_rest_of_the_engine() {
    // Rehearsal step 3 measured `masterClockFrame` stuck at 0 for several ticks
    // after `show.start` returned ok, then jumping to 150 — five seconds of
    // clock that had been running unobserved. The clock was never the defect:
    // `show.load` decodes the package inline on the async directive path, so
    // every later directive queues behind it and the telemetry pump, on the
    // same runtime, does not get to tick either.
    let ticks = Arc::new(AtomicU64::new(0));
    let ticker = {
        let ticks = ticks.clone();
        tokio::spawn(async move {
            loop {
                ticks.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
    };
    tokio::task::yield_now().await; // let the ticker reach its first sleep
    let before = ticks.load(Ordering::SeqCst);

    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path(), 6);
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());

    let started = Instant::now();
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    let load_took = started.elapsed();
    let after = ticks.load(Ordering::SeqCst);
    ticker.abort();

    // The load must be long enough for the question to mean something; if the
    // fixture ever decodes instantly this test proves nothing and should say so
    // rather than pass quietly.
    assert!(
        load_took > Duration::from_millis(20),
        "the fixture decoded in {load_took:?} — too fast to detect starvation; \
         add clips to `write_package`"
    );
    assert!(
        after > before,
        "another task made no progress during a {load_took:?} package load \
         ({before} -> {after} ticks): the decode is blocking the runtime, so \
         `show.start` cannot be applied and the telemetry pump cannot tick"
    );
}

/// The clock itself, isolated: applying `show.start` starts it immediately.
#[tokio::test]
async fn show_start_starts_the_clock_at_once() {
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path(), 1);
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    handler
        .apply(&directive("show.start", 2, serde_json::json!({})))
        .await
        .unwrap();
    assert!(
        state.clock.lock().unwrap().frame().is_some(),
        "the clock must be RUNNING the moment show.start is applied — if this \
         passes while the rehearsal sees 0, the delay is upstream of the clock"
    );
}

// ---------------------------------------------------------------------------
// R6 — every applied directive is acknowledged, and the ack is not held
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_applied_directive_is_acknowledged_not_only_the_resync() {
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path(), 1);
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());

    for (sv, cmd, payload) in [
        (
            1u64,
            "show.load",
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ),
        (2, "show.start", serde_json::json!({})),
        (3, "view.fallback", serde_json::json!({})),
    ] {
        handler.apply(&directive(cmd, sv, payload)).await.unwrap();
    }

    let acks: Vec<u64> = outgoing
        .drain()
        .into_iter()
        .filter_map(|f| match f {
            EngineFrame::AppliedStateVersion { state_version, .. } => Some(state_version),
            _ => None,
        })
        .collect();
    assert_eq!(
        acks,
        vec![1, 2, 3],
        "§5.9.5's quiescence handshake needs an ack per applied directive, in \
         order and with no gaps; got {acks:?}"
    );
    assert_eq!(state.last_applied(), 3);
}

// ---------------------------------------------------------------------------
// R6 — the ack reaches the wire promptly, not on the telemetry cadence
// ---------------------------------------------------------------------------

/// SPEC §5.9.5. The engine acked every directive all along; the acks waited in
/// `OutgoingQueue` for the outbound pump, and the pump looked at that queue once
/// per `telemetry_interval_ms`. Every existing wire test runs the channel at
/// `telemetry_interval_ms: 10`, which is why a one-second quantisation was
/// invisible to all of them — the harness was faster than the defect.
///
/// This runs at the production cadence and measures the ack's arrival against
/// it: a directive applied at t must be acknowledged well inside one telemetry
/// interval, or §5.9.5's two-second grace window is spending its budget on a
/// queue that is simply asleep.
#[tokio::test]
async fn an_ack_does_not_wait_for_the_telemetry_tick() {
    use futures_util::{SinkExt, StreamExt};
    use nbe_engine::channel::{self, EngineConfig};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    const INTERVAL_MS: u64 = 1000; // production cadence

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
        // Resync first: the connection gate refuses directives before one.
        let resync = serde_json::json!({
            "v": PROTOCOL_VERSION, "kind": "directive", "seq": 0, "stateVersion": 1,
            "command": nbe_protocol::command::RESYNC, "target": {},
            "payload": { "showState": "RUNNING", "viewItem": "A1", "previewItem": null,
                         "viewItemStartFrame": 0, "itemStates": {"A1": "LIVE"},
                         "sceneStates": {}, "visibleOverlays": [], "automationHold": false,
                         "fallbackActive": false, "stateVersion": 1 }
        });
        ws.send(Message::Text(resync.to_string().into()))
            .await
            .unwrap();

        // Drain whatever the resync produced, so the next ack we see is the
        // one this test times.
        let settle = tokio::time::Instant::now() + Duration::from_millis(200);
        while tokio::time::Instant::now() < settle {
            let _ = tokio::time::timeout(Duration::from_millis(20), ws.next()).await;
        }

        let take = serde_json::json!({
            "v": PROTOCOL_VERSION, "kind": "directive", "seq": 1, "stateVersion": 2,
            "command": "view.fallback", "target": {}, "payload": {}
        });
        let sent_at = Instant::now();
        ws.send(Message::Text(take.to_string().into()))
            .await
            .unwrap();

        // Wait for the ack for stateVersion 2, up to two telemetry intervals.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(INTERVAL_MS * 2);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(Ok(msg))) =
                tokio::time::timeout(Duration::from_millis(50), ws.next()).await
            {
                if !msg.is_text() {
                    continue;
                }
                let v: serde_json::Value = match serde_json::from_str(msg.to_text().unwrap()) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("kind").and_then(|k| k.as_str()) == Some("appliedStateVersion")
                    && v.get("stateVersion").and_then(|x| x.as_u64()) == Some(2)
                {
                    return Some(sent_at.elapsed());
                }
            }
        }
        None
    });

    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let cfg = EngineConfig {
        control_plane_url: format!("ws://{addr}/nbe/v0.3"),
        token: "t".into(),
        telemetry_interval_ms: INTERVAL_MS,
        ..Default::default()
    };
    let engine = tokio::spawn(channel::run_forever(cfg, state, outgoing));

    let latency = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("the test server finished")
        .expect("the test server did not panic");
    engine.abort();

    let latency = latency.expect("the engine acknowledged stateVersion 2 within two intervals");
    // A quarter of the interval is generous: the pump's own work is a drain and
    // a serialize. Anything near a full interval means the ack rode the tick.
    assert!(
        latency < Duration::from_millis(INTERVAL_MS / 4),
        "§5.9.5's ack arrived {latency:?} after the directive, against a \
         {INTERVAL_MS} ms telemetry interval — the pump is holding acks until \
         its next tick instead of draining when one is queued"
    );
}

// ---------------------------------------------------------------------------
// F1/F2 — an unattributable decode failure has an effect, not just a log line
// ---------------------------------------------------------------------------

/// A package whose only video asset no rundown Item references, and which
/// cannot decode.
fn write_orphan_failure_package(dir: &Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();
    std::fs::copy(media("corrupt.mp4"), dir.join("media/orphan.mp4")).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.4",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" },
                { "id": "orphan", "kind": "video", "source": "media/orphan.mp4", "format": "h264" }
            ],
            // The scene shows the slate. Nothing references `orphan`, so its
            // decode failure is attributable to no Item.
            "scenes": [{ "id": "SCN", "elements": [
                { "id": "main", "kind": "graphic", "z": 1, "templateId": "TPL" }
            ]}],
            "templates": [{ "id": "TPL", "kind": "generic" }],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn an_unattributable_decode_failure_is_counted_not_only_logged() {
    // F1: `directive.rs`'s `affected.is_empty()` branch was a `warn!` and
    // nothing else — delete the line and the suite stayed green, which made it
    // the one place "logged, therefore not swallowed" rested on an ungated
    // line. F2: `VideoLibrary::failures` was written and read nowhere.
    //
    // Standing invariant 3: a gate observes an effect, not text. The effect is
    // the count — the only evidence this happened, since no Item will go ERROR
    // for an asset no Item references.
    let dir = tempfile::tempdir().unwrap();
    write_orphan_failure_package(dir.path());
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();

    assert_eq!(
        state.decode_failures_total.load(Ordering::Relaxed),
        1,
        "the failure must be counted"
    );
    assert_eq!(
        state
            .unattributable_decode_failures_total
            .load(Ordering::Relaxed),
        1,
        "and counted as unattributable, because no rundown Item names `orphan`"
    );
    // And it produced no `itemEvent`: there is no Item to address one to.
    let events = outgoing.drain();
    assert!(
        !events
            .iter()
            .any(|f| matches!(f, EngineFrame::ItemEvent { .. })),
        "an unattributable failure must not invent an Item to blame"
    );
}
