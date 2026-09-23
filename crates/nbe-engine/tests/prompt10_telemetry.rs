//! Prompt 10 WU6 (SPEC §10.1 + §10.1.1): telemetry conformance for
//! `streamState` (the control plane's, "as commanded") + `streamBufferMs`
//! (the render node's).
//!
//! TDD: written BEFORE any WU6 implementation change (RED first). The engine
//! half of the split lives here; the control-plane half (`streamState`
//! presence + token stability + parse-and-forward readability) lives in
//! `packages/control-plane/src/telemetry.test.ts`. Together they assert the
//! DoD: both fields present in EVERY emitted tick (idle pre-start, live,
//! stopped) with lawful stubs.
//!
//! Ownership (§10.1's table, §10.1.1): the engine NEVER emits `streamState` —
//! it is control-plane state, "as commanded". What the engine owes the wire
//! is `streamBufferMs` on every tick: `-1.0` with no session (the NO-SESSION
//! sentinel — negative ms is impossible, so it never collides with an honest
//! drained-live `0.0`), the session's transport counter while live, `-1.0`
//! again after stop. No new wire field is introduced here (zero new fields
//! preferred); a missing key below fails rather than skips.
//!
//! Rule 7: every test that enters the streaming path drives a REAL command
//! through `DirectiveHandler` (`show.load` → `show.start` → `stream.start` /
//! `stream.stop`); no test writes `stream_state` or a session into
//! `EngineState` directly. The pre-start test enters nothing by construction.
//! Rule 8: hardware-gated tests skip loudly (`SKIP:` + return) so the gate
//! counts `exercised = ran - skipped`.

use std::sync::Arc;

use nbe_engine::directive::{DirectiveError, DirectiveHandler};
use nbe_engine::state::{EngineState, OutgoingQueue, StreamState};
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION};

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

fn harness() -> (Arc<EngineState>, DirectiveHandler, Arc<OutgoingQueue>) {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    (state, handler, outgoing)
}

fn hw_or_skip() -> bool {
    if nbe_engine::record::encoder_available() {
        return true;
    }
    eprintln!("SKIP: no hardware H.264 encoder on this machine (SPEC §9.2); live-tick tests need the encoder");
    false
}

async fn chain_or_skip(state: &Arc<EngineState>) -> bool {
    let _render = nbe_engine::render::RenderLoop::new(state.clone())
        .await
        .ok();
    if nbe_engine::record::stream::chain_available(&state.render_device()) {
        return true;
    }
    eprintln!("SKIP: no zero-copy chain on this machine; streaming has no lawful path (§0.1 assumption 24)");
    false
}

/// A minimal loadable package; `stream_url = None` declares no stream output.
fn write_package(stream_url: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf) {
    let pkg = tempfile::tempdir().expect("package tempdir must succeed");
    std::fs::create_dir_all(pkg.path().join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(pkg.path().join("media/slate.png"), &png).unwrap();
    let stream = match stream_url {
        Some(u) => serde_json::json!({ "url": u }),
        None => serde_json::json!({}),
    };
    std::fs::write(
        pkg.path().join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.4",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate",
                "outputs": { "stream": stream }
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" }
            ],
            "scenes": [],
            "rundown": { "id": "R", "items": [] },
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
    let pkg_path = pkg.path().to_path_buf();
    (pkg, pkg_path)
}

async fn load_and_start(handler: &DirectiveHandler, pkg_path: &std::path::Path) {
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": pkg_path.to_string_lossy() }),
        ))
        .await
        .expect("show.load of the test package must succeed");
    handler
        .apply(&directive("show.start", 2, serde_json::json!({})))
        .await
        .unwrap();
}

/// The tick's `streamBufferMs` wire value: the struct field AND the serialized
/// JSON key. A field that serializes to nothing is absent on the wire (§10.1.1
/// forbids exactly that), so both are asserted together, on every tick.
fn tick_stream_buffer_ms(state: &Arc<EngineState>) -> (f64, serde_json::Value) {
    let frame = nbe_engine::telemetry::build_tick(state);
    let value = serde_json::to_value(&frame).expect("telemetry frame must serialize");
    let ms = match &frame {
        EngineFrame::EngineTelemetry { fields, .. } => fields.stream_buffer_ms,
        _ => panic!("build_tick must emit engineTelemetry"),
    };
    (ms, value)
}

fn assert_wire_key_present(value: &serde_json::Value) {
    let fields = value
        .get("streamBufferMs")
        .unwrap_or_else(|| panic!("every engine tick must carry streamBufferMs, got: {value}"));
    assert!(
        fields.is_number(),
        "streamBufferMs must be a number on the wire, got: {fields}"
    );
}

fn is_bad_payload(err: &DirectiveError) -> bool {
    matches!(err, DirectiveError::Invalid(msg) if msg.contains("E_BAD_PAYLOAD"))
}

// ---------------------------------------------------------------------------
// Idle pre-start: the lawful stub (always runs — no hardware needed).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prestart_tick_carries_stream_buffer_ms_stub_not_absence() {
    // Fresh engine, no show, no session: the tick must still carry the key,
    // stubbed at -1.0 — an absent field and a stubbed field are different
    // failures and only one of them is diagnosable (§10.1.1). The sentinel is
    // negative because 0.0 is a legal drained-live value (v0.4.4 stub rule).
    let (state, _handler, _) = harness();
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "a fresh engine is Idle"
    );
    assert!(state.stream_session.lock().unwrap().is_none());

    let (ms, value) = tick_stream_buffer_ms(&state);
    assert_wire_key_present(&value);
    assert_eq!(
        ms, -1.0,
        "pre-start streamBufferMs must stub at -1.0 (no session to measure)"
    );
    assert_eq!(
        value.get("streamBufferMs").and_then(|v| v.as_f64()),
        Some(-1.0),
        "the -1.0 sentinel must be distinguishable on the wire, not just in the struct"
    );

    // A drained-live session honestly reports 0.0 — the two wire values must
    // differ. Standalone session (never inserted into EngineState, per Rule 7):
    // a non-RTMP endpoint spawns no publisher, so the counter reads drained.
    let drained = nbe_engine::record::stream::StreamSession::open(
        "not-a-publish-target",
        nbe_engine::record::tap_path::Selection {
            path: nbe_engine::record::tap_path::TapPath::ZeroCopy,
            reason: nbe_engine::record::tap_path::Reason::Table,
        },
    );
    assert_eq!(
        drained.stream_buffer_ms(),
        0.0,
        "a transport-less (drained) session reports an honest 0.0"
    );
    assert_ne!(
        ms,
        drained.stream_buffer_ms(),
        "NO-SESSION (-1.0) and drained-live (0.0) must be distinguishable on the wire"
    );
}

// ---------------------------------------------------------------------------
// Refused start via a REAL command (hardware-free: the endpoint check
// precedes the encoder and chain probes): the tick stays lawful.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refused_start_leaves_a_lawful_stub_tick() {
    let (state, handler, _) = harness();
    let (_pkg, pkg_path) = write_package(None);
    load_and_start(&handler, &pkg_path).await;

    let err = handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect_err("stream.start with no endpoint anywhere must refuse");
    assert!(
        is_bad_payload(&err),
        "expected E_BAD_PAYLOAD-shaped refusal, got: {err}"
    );
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "refused start must leave the state machine Idle"
    );
    assert!(state.stream_session.lock().unwrap().is_none());
    assert!(
        state.stream_tap_selection.lock().unwrap().is_none(),
        "refused start publishes no selection"
    );

    let (ms, value) = tick_stream_buffer_ms(&state);
    assert_wire_key_present(&value);
    assert_eq!(
        ms, -1.0,
        "a refused start must leave the -1.0 sentinel, never an absent field"
    );
}

// ---------------------------------------------------------------------------
// Live → stopped via REAL commands (hardware-gated, skips loudly): the live
// tick wires the session's transport counter exactly, and stop returns the
// tick to the stub with the key still present.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn live_tick_wires_the_session_counter_and_stop_returns_to_stub() {
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"));
    load_and_start(&handler, &pkg_path).await;

    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .unwrap_or_else(|e| panic!("stream.start with a manifest endpoint must open, got: {e}"));
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Live,
        "open start must flip to Live"
    );

    // The tick IS the transport counter, not a constant: whatever the session
    // reports — 0.0 drained, nonzero under load — the tick reports identically.
    let session_ms = state
        .stream_session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.stream_buffer_ms())
        .unwrap_or(f64::NAN);
    let (tick_ms, value) = tick_stream_buffer_ms(&state);
    assert_wire_key_present(&value);
    assert_eq!(
        tick_ms, session_ms,
        "live tick streamBufferMs must equal the session counter, not a constant"
    );

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("stop of a live stream must succeed");
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "stop must return the state machine to Idle"
    );
    assert!(state.stream_session.lock().unwrap().is_none());

    let (ms, value) = tick_stream_buffer_ms(&state);
    assert_wire_key_present(&value);
    assert_eq!(
        ms, -1.0,
        "stopped tick must return to the -1.0 sentinel with the key still present"
    );
}
