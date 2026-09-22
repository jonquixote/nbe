//! Prompt 10 WU4 (SPEC §16.14, §16.1, §9.1): engine `stream.start` /
//! `stream.stop` command handling + `show.stop` quiescence.
//!
//! TDD: written BEFORE the `stream.*` arms land (RED first). Every test below
//! drives the REAL directive path (`show.load` with a real package →
//! `show.start` → `stream.start` / `stream.stop` / `show.stop`) and reads
//! selection/session state back out — never state writes (rule 7).
//!
//! Coverage maps to the WU4 DoD:
//! 1. `stream.start` on a RUNNING show with an endpoint (manifest url in the
//!    test package) opens a stream session (state flips, selection published
//!    via the `tap_path` `select_stream` path).
//! 2. Preconditions, each tested: not-running → `E_FORBIDDEN_STATE`; no
//!    endpoint anywhere → `E_BAD_PAYLOAD` (via `resolve_stream_url`); a
//!    chain-less machine (forced via the `force_no_chain` seam — the mirror of
//!    record's `force_no_encoder`) → `E_NO_ZEROCOPY` refusal, loudly.
//! 3. `stream.stop` requires an active stream (`E_FORBIDDEN_STATE` while idle);
//!    stop finalizes before the ack flows (session closed + `last_applied`
//!    advanced only after — the `record_stop_failure_withholds_ack_and_keeps_file`
//!    shape); a failed teardown withholds the ack.
//! 4. `show.stop` quiescence: graceful (active stream stops, ack after
//!    teardown), force path (warns + stops anyway), `quiesceOutputs=false` +
//!    no force refused — the record quiescence table shape.
//! 5. The stream `tapPath` manifest field flows into the selection via
//!    `select_with_override` (the stream side of B2).
//! 6. A second `stream.start` while live → `E_FORBIDDEN_STATE` (ceiling:
//!    exactly one live stream, §9.1).

use std::sync::Arc;

use nbe_engine::directive::{DirectiveError, DirectiveHandler};
use nbe_engine::record::stream as stream_glue;
use nbe_engine::record::tap_path::{Consumer, TapPath};
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, OutgoingQueue, StreamState};
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION};

/// The stream tests share process-wide seams (the force-no-chain flag and the
/// force-close-error flag), so they serialize on this lock (cf.
/// `prompt09_session.rs`).
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

fn is_forbidden(err: &DirectiveError) -> bool {
    matches!(err, DirectiveError::ForbiddenState(_))
        && err.to_string().contains("E_FORBIDDEN_STATE")
}

fn is_bad_payload(err: &DirectiveError) -> bool {
    matches!(err, DirectiveError::Invalid(msg) if msg.contains("E_BAD_PAYLOAD"))
}

fn is_no_zerocopy(err: &DirectiveError) -> bool {
    err.to_string().contains("E_NO_ZEROCOPY")
}

fn acked(outgoing: &OutgoingQueue, sv: u64) -> bool {
    outgoing.drain().into_iter().any(|f| match f {
        EngineFrame::AppliedStateVersion { state_version, .. } => state_version == sv,
        _ => false,
    })
}

fn hw_or_skip() -> bool {
    if nbe_engine::record::encoder_available() {
        return true;
    }
    eprintln!(
        "SKIP: no hardware H.264 encoder on this machine (SPEC §9.2); stream.start cannot open"
    );
    false
}

/// Resets the force-no-chain seam on drop so a panicking test cannot poison
/// later tests in this binary (the `ForceNoEncoderGuard` shape).
struct ForceNoChainGuard;
impl ForceNoChainGuard {
    fn set() -> Self {
        stream_glue::set_force_no_chain(true);
        Self
    }
}
impl Drop for ForceNoChainGuard {
    fn drop(&mut self) {
        stream_glue::set_force_no_chain(false);
    }
}

/// Resets the force-close-error seam on drop.
struct ForceCloseErrorGuard;
impl ForceCloseErrorGuard {
    fn set() -> Self {
        stream_glue::set_force_close_error(true);
        Self
    }
}
impl Drop for ForceCloseErrorGuard {
    fn drop(&mut self) {
        stream_glue::set_force_close_error(false);
    }
}

/// A minimal loadable package whose stream output carries `url` and, when
/// given, `tapPath`. Returns the package dir (for show.load).
fn write_package(
    stream_url: Option<&str>,
    tap: Option<&str>,
) -> (tempfile::TempDir, std::path::PathBuf) {
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
    let mut stream = serde_json::json!({});
    if let Some(u) = stream_url {
        stream["url"] = u.into();
    }
    if let Some(t) = tap {
        stream["tapPath"] = t.into();
    }
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

/// Publish the render device where there is one, then ask the PRODUCTION probe
/// whether this machine has a lawful streaming chain. A headless box with no
/// chain skips loudly (rule 8) instead of failing — the refusal row itself is
/// pinned hardware-free by `stream_start_on_chain_less_machine_refuses`.
async fn chain_or_skip(state: &Arc<EngineState>) -> bool {
    let _render = RenderLoop::new(state.clone()).await.ok();
    if stream_glue::chain_available(&state.render_device()) {
        return true;
    }
    eprintln!("SKIP: no zero-copy chain on this machine; streaming has no lawful path (§0.1 assumption 24)");
    false
}

fn endpoint_of(state: &Arc<EngineState>) -> String {
    state
        .stream_session
        .lock()
        .unwrap()
        .as_ref()
        .expect("stream session must be present")
        .endpoint()
        .to_string()
}

fn selection_of(state: &Arc<EngineState>) -> nbe_engine::record::tap_path::Selection {
    state
        .stream_tap_selection
        .lock()
        .unwrap()
        .expect("a started stream must have published its selection")
}

/// The stream is really live — not just "the command returned Ok" (unknown
/// commands are ignored + acked, so Ok alone proves nothing).
fn assert_live(state: &Arc<EngineState>) {
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Live,
        "stream.start must have flipped to Live"
    );
    assert!(
        state.stream_session.lock().unwrap().is_some(),
        "stream.start must have opened a session"
    );
}

// ---------------------------------------------------------------------------
// 1. Flagship — stream.start on a RUNNING show with a manifest endpoint opens.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_start_on_running_show_with_manifest_url_opens_session() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), None);
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
    assert_eq!(
        endpoint_of(&state),
        "rtmp://manifest.example/live",
        "the manifest endpoint answers a silent command (WU3 call site)"
    );
    let sel = selection_of(&state);
    assert_eq!(
        sel.path,
        TapPath::ZeroCopy,
        "a capable stream runs zero-copy (select_stream path, never readback)"
    );
    assert_eq!(state.last_applied(), 3);
    assert!(
        acked(&outgoing, 3),
        "start must ack after the session opened"
    );

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// 2. Preconditions.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_start_without_running_show_is_forbidden() {
    let _serial = SERIAL.lock().await;
    let (state, handler, outgoing) = harness();
    assert!(!state.is_running(), "clock starts stopped");

    let err = handler
        .apply(&directive(
            "stream.start",
            1,
            serde_json::json!({"url": "rtmp://command.example/live"}),
        ))
        .await
        .expect_err("stream.start with no RUNNING show must be refused");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "refused start must not mutate stream state"
    );
    assert!(state.stream_session.lock().unwrap().is_none());
    assert!(!acked(&outgoing, 1), "refused start must not ack");
}

#[tokio::test]
async fn stream_start_with_neither_url_refuses_bad_payload() {
    // No endpoint anywhere (package declares no stream url, command silent) →
    // E_BAD_PAYLOAD via resolve_stream_url. Hardware-free: the endpoint check
    // precedes the encoder and chain probes.
    let _serial = SERIAL.lock().await;
    let (state, handler, outgoing) = harness();
    let (_pkg, pkg_path) = write_package(None, None);
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
    assert!(!acked(&outgoing, 3), "refused start must not ack");
}

#[tokio::test]
async fn stream_start_on_chain_less_machine_refuses_no_zerocopy_loudly() {
    // SPEC §16.14 precondition, the record force-no-encoder shape mirrored: a
    // forced-unavailable chain behaves exactly like a chain-less machine —
    // refused with E_NO_ZEROCOPY in the message (loud, never silent), with no
    // session and no ack. v0.4.2's readback allowance is recording-only, so
    // there is no CPU fallback here.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let _force = ForceNoChainGuard::set();
    let (state, handler, outgoing) = harness();
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), None);
    load_and_start(&handler, &pkg_path).await;

    let err = handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect_err("stream.start with no zero-copy chain must refuse");

    assert!(
        is_no_zerocopy(&err),
        "expected E_NO_ZEROCOPY token in the refusal, got: {err}"
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
    assert!(!acked(&outgoing, 3), "refused start must not ack");
}

// ---------------------------------------------------------------------------
// 3. stream.stop: requires live, finalizes before ack, failure withholds ack.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_stop_while_idle_is_forbidden() {
    let _serial = SERIAL.lock().await;
    let (state, handler, _) = harness();

    let err = handler
        .apply(&directive("stream.stop", 1, serde_json::json!({})))
        .await
        .expect_err("stream.stop with no live stream must be refused");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "refused stop must not mutate stream state"
    );
}

#[tokio::test]
async fn stream_stop_finalizes_before_ack_flows() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("stream.stop on a live stream must close it");

    // Ordering proof: the teardown ran BEFORE the ack became observable — the
    // session is gone and applied advanced to the stop together.
    assert_eq!(state.last_applied(), 4);
    assert!(acked(&outgoing, 4), "stop must ack after teardown ran");
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "stop must return the state machine to Idle"
    );
    assert!(
        state.stream_session.lock().unwrap().is_none(),
        "stop must consume the session"
    );
}

#[tokio::test]
async fn stream_stop_failure_withholds_ack() {
    // The record_stop_failure_withholds_ack_and_keeps_file shape: a failed
    // teardown surfaces (no ack, applied frozen) and the live state still ends
    // (no pipeline remains to continue with).
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    let _force = ForceCloseErrorGuard::set();
    let err = handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect_err("a failed teardown must surface, never ack");

    assert!(
        err.to_string().contains("E_NETWORK"),
        "expected E_NETWORK, got: {err}"
    );
    assert_ne!(
        state.last_applied(),
        4,
        "failed stop must not advance applied"
    );
    assert!(!acked(&outgoing, 4), "failed stop must not ack");
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "failed teardown still ends the live state"
    );
    assert!(state.stream_session.lock().unwrap().is_none());
}

// ---------------------------------------------------------------------------
// 4. show.stop quiescence — the record table shape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn show_stop_gracefully_quiesces_a_live_stream() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    // No stream.stop: show.stop alone must quiesce the live stream, and the
    // ack follows the teardown (the stop's version, not the start's).
    handler
        .apply(&directive("show.stop", 4, serde_json::json!({})))
        .await
        .expect("show.stop must succeed while streaming");

    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "quiesced stream must be Idle"
    );
    assert!(state.stream_session.lock().unwrap().is_none());
    assert!(
        !state.is_running(),
        "quiesced stop still stops the show clock"
    );
    assert_eq!(
        state.last_applied(),
        4,
        "ack version for the stop must be the stop directive's version"
    );
    assert!(acked(&outgoing, 4));
}

#[tokio::test]
async fn show_stop_force_stops_a_live_stream_anyway() {
    // §16.1 table (quiesceOutputs=true, force=true): immediate stop — the
    // session is dropped as-is, a warning is logged, the show stops, and the
    // stop acks (force was requested).
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    handler
        .apply(&directive(
            "show.stop",
            4,
            serde_json::json!({"quiesceOutputs": true, "force": true}),
        ))
        .await
        .expect("forced show.stop must stop immediately");

    assert_eq!(*state.stream_state.lock().unwrap(), StreamState::Idle);
    assert!(state.stream_session.lock().unwrap().is_none());
    assert!(
        !state.is_running(),
        "forced stop still stops the show clock"
    );
    assert!(acked(&outgoing, 4));
}

#[tokio::test]
async fn show_stop_with_quiesce_outputs_false_is_refused_while_live() {
    // §16.1 table (quiesceOutputs=false, force=false): fail with
    // E_FORBIDDEN_STATE — the session is NOT torn down, the show keeps running.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    let err = handler
        .apply(&directive(
            "show.stop",
            4,
            serde_json::json!({"quiesceOutputs": false}),
        ))
        .await
        .expect_err("show.stop must not quiesce when told not to");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Live,
        "refused quiesce must leave the stream live"
    );
    assert!(
        state.stream_session.lock().unwrap().is_some(),
        "refused quiesce must not consume the session"
    );
    assert!(state.is_running(), "refused stop keeps the show clock");
    assert!(!acked(&outgoing, 4), "refused stop must not ack");

    // Cleanup: the stream is still live, so a real stop still works.
    handler
        .apply(&directive("stream.stop", 5, serde_json::json!({})))
        .await
        .expect("the preserved stream must still stop cleanly");
}

#[tokio::test]
async fn show_stop_with_quiesce_outputs_false_and_force_stops_immediately() {
    // §16.1 table (quiesceOutputs=false, force=true): immediate stop — the
    // session is dropped, the show stops, the stop acks.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    handler
        .apply(&directive(
            "show.stop",
            4,
            serde_json::json!({"quiesceOutputs": false, "force": true}),
        ))
        .await
        .expect("forced stop without quiesce must stop immediately");

    assert_eq!(*state.stream_state.lock().unwrap(), StreamState::Idle);
    assert!(!state.is_running());
    assert!(acked(&outgoing, 4));
}

// ---------------------------------------------------------------------------
// 5. Stream tapPath flows into the selection (B2 stream side).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_tap_path_cpu_readback_flows_into_selection() {
    // Record-side precedent (WU2): the manifest's tapPath override reaches the
    // session via select_with_override and reports Override.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), Some("cpuReadback"));
    load_and_start(&handler, &pkg_path).await;

    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    let sel = selection_of(&state);
    assert_eq!(
        sel.path,
        TapPath::CpuReadback,
        "a cpuReadback stream take runs the override path"
    );
    assert_eq!(
        sel.reason,
        nbe_engine::record::tap_path::Reason::Override,
        "the restriction must say Override: an operator action, not the table"
    );
    // The override consulted the stream consumer row, not the record one.
    let expected = nbe_engine::record::tap_path::select_with_override(
        true,
        nbe_engine::render::VIEW_H,
        Consumer::Stream,
        Some(TapPath::CpuReadback),
    );
    assert_eq!(sel, expected);

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// 6. Ceiling: exactly one live stream (§9.1).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn second_stream_start_while_live_is_forbidden() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("first stream.start must open");
    let first_endpoint = endpoint_of(&state);

    let err = handler
        .apply(&directive(
            "stream.start",
            4,
            serde_json::json!({"url": "rtmp://other.example/live"}),
        ))
        .await
        .expect_err("second stream.start while live must be refused");

    assert!(is_forbidden(&err), "expected E_FORBIDDEN_STATE, got: {err}");
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Live,
        "refused second start must leave the stream live"
    );
    assert_eq!(
        endpoint_of(&state),
        first_endpoint,
        "refused second start must not replace the live session"
    );

    handler
        .apply(&directive("stream.stop", 5, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}
