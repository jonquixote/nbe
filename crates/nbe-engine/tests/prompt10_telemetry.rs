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
//! is `streamBufferMs` on every tick: **`-1.0` with no session** — the
//! NO-SESSION sentinel, "no measurement exists", ratified in SPEC v0.4.6 —
//! the session's transport counter while live (`>= 0.0`; `0.0` is "the buffer
//! is empty"), and `-1.0` again after stop. A missing key below fails rather
//! than skips. v0.4.6 also adds `streamTransportState` (tests at the end).
//!
//! History, kept per §2c — this suite has pinned both values:
//!
//! * ~~`-1.0` with no session (the NO-SESSION sentinel)~~ — PR #30's first
//!   version. It changed the meaning of a ratified field inside a feature PR;
//!   reverted in the repair round (`f8ff895`) and drafted as an UNRATIFIED
//!   candidate in `docs/v0.5-outline.md` §7.
//! * ~~`0.0` with no session (nothing is buffered — §10.1's meaning, measured
//!   rather than stubbed) … `0.0` again after stop. Idle vs drained-live is
//!   `streamState`'s to say.~~ — the repair round's version, which these
//!   tests pinned from `f8ff895` (the F13 falsification) until v0.4.6. §10.1
//!   never stated a no-session value; v0.4.6 states `-1.0`, and the three
//!   tests below now pin the `-1.0` / `0.0` distinction instead of `0.0`.
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
use nbe_protocol::{
    DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION, STREAM_BUFFER_NO_SESSION_MS,
};

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
    // Fresh engine, no show, no session: the tick must still carry the key
    // (§10.1.1: an absent field and a stubbed field are different failures
    // and only one of them is diagnosable), at -1.0 — no measurement exists
    // (v0.4.6). ~~"at 0.0 — no buffer holds nothing"~~ (the repair round's
    // pin, retired §2c).
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
        ms, STREAM_BUFFER_NO_SESSION_MS,
        "pre-start streamBufferMs is -1.0: no session, so no measurement exists"
    );
    assert_eq!(
        value.get("streamBufferMs").and_then(|v| v.as_f64()),
        Some(-1.0),
        "the sentinel must reach the wire as -1, not just the struct"
    );
    assert_ne!(
        ms, 0.0,
        "no-session must not read as a live session's empty buffer"
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
        ms, STREAM_BUFFER_NO_SESSION_MS,
        "a refused start opened no session: -1.0 on the wire, never an absent \
         field and never 0.0 (~~\"leaves 0.0 on the wire\"~~, retired v0.4.6)"
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
    // Keyed since PR #33's fix round: resolution now refuses a keyless URL
    // (~~`rtmp://manifest.example/live`~~, which opened a publisher-less
    // session — the false-live, §2c).
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"));
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
    // And with a session it is a MEASUREMENT (>= 0.0), never the sentinel.
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
    assert!(
        tick_ms >= 0.0,
        "a live session is measured (>= 0.0), never the no-session sentinel, got {tick_ms}"
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
        ms, STREAM_BUFFER_NO_SESSION_MS,
        "stopped tick returns to the -1.0 sentinel with the key still present \
         (~~\"returns to 0.0\"~~, retired v0.4.6)"
    );
}

// ---------------------------------------------------------------------------
// `streamTransportState` (SPEC §10.1, ratified v0.4.6): the transport's own
// state, always on the wire. The completeness assertion runs everywhere; the
// live leg is hardware-gated and skips loudly.
// ---------------------------------------------------------------------------

/// The tick's `streamTransportState`, read off the SERIALIZED frame: §10.1.1's
/// "a consumer must never see a missing field" is a claim about the wire, so a
/// `#[serde(skip_serializing)]` that left the struct intact must still fail.
fn tick_transport_state(state: &Arc<EngineState>) -> String {
    let frame = nbe_engine::telemetry::build_tick(state);
    let value = serde_json::to_value(&frame).expect("telemetry frame must serialize");
    let obj = value
        .as_object()
        .expect("an engineTelemetry frame is an object");
    let token = obj.get("streamTransportState").unwrap_or_else(|| {
        panic!(
            "§10.1.1: every engine tick must carry streamTransportState; keys were {:?}",
            obj.keys().collect::<Vec<_>>()
        )
    });
    token
        .as_str()
        .unwrap_or_else(|| panic!("streamTransportState must be a string, got: {token}"))
        .to_string()
}

#[tokio::test]
async fn the_transport_field_is_always_on_the_wire_and_stubs_before_any_stream_starts() {
    // Fresh engine: the stub, present.
    let (state, handler, _) = harness();
    assert_eq!(
        tick_transport_state(&state),
        "none",
        "before any stream has started the field is the stub, never absent"
    );

    // A refused start (hardware-free: E_BAD_PAYLOAD precedes both probes)
    // opens no transport, so the stub stands.
    let (_pkg, pkg_path) = write_package(None);
    load_and_start(&handler, &pkg_path).await;
    let err = handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect_err("stream.start with no endpoint anywhere must refuse");
    assert!(is_bad_payload(&err), "expected E_BAD_PAYLOAD, got: {err}");
    assert_eq!(
        tick_transport_state(&state),
        "none",
        "a refused start opened no transport; the stub stands"
    );
}

#[tokio::test]
async fn live_tick_carries_the_transport_state_and_stop_reads_closed() {
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    // `manifest.example` does not resolve, so the publisher never completes a
    // dial: the one transport state this leg can pin without an ingest.
    //
    // ~~The KEY matters: `rtmp://manifest.example/live` (the other live test's
    // URL) is keyless, `parse_rtmp_url` refuses it, and the session opens with
    // no publisher at all — `streamState` live, nothing published, and this
    // field reading `"closed"`. That is a pre-existing defect
    // (`resolve_stream_url` checks only the scheme), recorded in the prompt
    // map's v0.4.6 entry, and exactly the kind of thing the field exists to
    // expose.~~ FIXED in PR #33's fix round (§2c): `resolve_stream_url` now
    // runs the publisher's own parse, so a keyless URL is refused
    // `E_BAD_PAYLOAD` before any session exists
    // (`stream_url_precedence::a_keyless_rtmp_url_refuses_bad_payload_and_names_the_key`).
    // This field is how the defect was found: it read `"closed"` beside a
    // live stream.
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .unwrap_or_else(|e| panic!("stream.start with a manifest endpoint must open, got: {e}"));

    let session_token = state
        .stream_session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.publisher_state().as_str())
        .expect("a live stream has a session");
    assert_eq!(
        tick_transport_state(&state),
        session_token,
        "the tick carries the session's own transport state"
    );
    assert_eq!(
        session_token, "reconnecting",
        "an unresolvable ingest is a dial in progress"
    );
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Live,
        "while the transport redials the engine's streamState stays Live (§9.5)"
    );

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("stop of a live stream must succeed");
    assert_eq!(
        tick_transport_state(&state),
        "closed",
        "after stop the socket is gone: closed, not the never-started stub"
    );
}
