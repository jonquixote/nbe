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
//! 5. The stream `tapPath` manifest field is READ but `cpuReadback` for
//!    streaming is refused `E_NO_ZEROCOPY` (no lawful readback path — never an
//!    Override-live selection).
//! 6. A second `stream.start` while live → `E_FORBIDDEN_STATE` (ceiling:
//!    exactly one live stream, §9.1).
//! 7. `url` validation: garbage-scheme and non-string urls refuse
//!    `E_BAD_PAYLOAD` (hardware-free); an uppercase `RTMP://` scheme resolves.
//! 8. Fresh counters per stream (`skipped_stream_frames` + `stream_tap_ms`
//!    zeroed at start, the record-start shape) and the selection clears on
//!    every stop path.
//! 9. `show.stop` with both outputs live quiesces inside the §16.1 2 s window
//!    (concurrent teardowns, first failure wins) and a failing stream alone
//!    withholds the ack.
//! 10. A `stream.stop` racing the feed leg's session lock returns boundedly
//!     (session-before-state order — never a deadlock).

use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::directive::{DirectiveError, DirectiveHandler};
use nbe_engine::record::stream as stream_glue;
use nbe_engine::record::tap_path::TapPath;
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

fn is_no_encoder(err: &DirectiveError) -> bool {
    err.to_string().contains("E_NO_HARDWARE_ENCODER")
}

/// Resets the force-no-encoder seam on drop (the prompt09_session shape), so a
/// panicking test cannot leave the process believing it has no encoder.
struct ForceNoEncoderGuard;
impl ForceNoEncoderGuard {
    fn set() -> Self {
        nbe_engine::record::session::set_force_no_encoder(true);
        Self
    }
}
impl Drop for ForceNoEncoderGuard {
    fn drop(&mut self) {
        nbe_engine::record::session::set_force_no_encoder(false);
    }
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

/// Package variant with a record target (absolute tempdir path) so
/// `record.start` with `{}` opens a take there. Returns the package tempdir
/// (kept alive), the package path, and the record dir tempdir (kept alive).
fn write_record_package(
    stream_url: Option<&str>,
    tap: Option<&str>,
) -> (tempfile::TempDir, std::path::PathBuf, tempfile::TempDir) {
    let rec = tempfile::tempdir().expect("record tempdir must succeed");
    let (pkg, pkg_path) = write_package(stream_url, tap);
    let manifest_path = pkg_path.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["show"]["outputs"]["record"] =
        serde_json::json!({ "directory": rec.path().to_string_lossy() });
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    (pkg, pkg_path, rec)
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
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
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
        "rtmp://manifest.example/live/key",
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
            serde_json::json!({"url": "rtmp://command.example/live/key"}),
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
    //
    // UNGATED (PR #30 repair round): the chain refusal now precedes the
    // encoder probe, so this runs on the encoder-less CI runner too. It used to
    // `hw_or_skip()` first — which meant CI reported it `ok` while never
    // exercising it (§2a rule 8).
    let _serial = SERIAL.lock().await;
    let _force = ForceNoChainGuard::set();
    let (state, handler, outgoing) = harness();
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
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

/// The refusal ORDER is part of the contract, so it is pinned (PR #30 repair
/// round): configuration, then the chain, then the encoder — **SPEC §16.14
/// law since v0.4.6**, and this test is its guard. (~~"then the SPEC's chain
/// refusal, then this build's encoder"~~ — §9.2's hardware-only encode is
/// spec law too.) The first version probed the encoder first; on the macos-14 runner
/// (Metal adapter, no H.264 encoder) that made every chain and configuration
/// refusal unreachable, and CI run 35878301689 failed on exactly that.
///
/// The three legs, each isolating one layer with the force seams:
/// * no encoder AND no chain -> `E_NO_ZEROCOPY` (chain before encoder);
/// * no encoder AND a `cpuReadback` manifest -> `E_NO_ZEROCOPY` naming
///   `cpuReadback` (configuration before both probes);
/// * no encoder, real chain -> `E_NO_HARDWARE_ENCODER` (the encoder is still
///   refused when it is the only thing missing). This leg needs a real chain
///   and skips loudly without one; the macos-14 runner has one.
#[tokio::test]
async fn stream_start_refusal_order_is_config_then_chain_then_encoder() {
    let _serial = SERIAL.lock().await;
    let _no_encoder = ForceNoEncoderGuard::set();

    // Leg 1: both probes would refuse; the chain's answer wins.
    {
        let _no_chain = ForceNoChainGuard::set();
        let (_state, handler, _) = harness();
        let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
        load_and_start(&handler, &pkg_path).await;
        let err = handler
            .apply(&directive("stream.start", 3, serde_json::json!({})))
            .await
            .expect_err("no chain and no encoder must refuse");
        assert!(
            is_no_zerocopy(&err) && !is_no_encoder(&err),
            "chain refusal must precede the encoder probe, got: {err}"
        );
    }

    // Leg 2: a cpuReadback stream is refused before either probe.
    {
        let (_state, handler, _) = harness();
        let (_pkg, pkg_path) = write_package(
            Some("rtmp://manifest.example/live/key"),
            Some("cpuReadback"),
        );
        load_and_start(&handler, &pkg_path).await;
        let err = handler
            .apply(&directive("stream.start", 3, serde_json::json!({})))
            .await
            .expect_err("a cpuReadback stream must refuse");
        let msg = err.to_string();
        assert!(
            is_no_zerocopy(&err) && msg.contains("cpuReadback"),
            "the configuration refusal must precede both probes and name the setting, got: {msg}"
        );
    }

    // Leg 3: the encoder is refused when it is the only thing missing.
    let (state, handler, _) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
    load_and_start(&handler, &pkg_path).await;
    let err = handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect_err("no encoder must refuse even with a chain");
    assert!(
        is_no_encoder(&err),
        "with a chain present, a missing encoder answers E_NO_HARDWARE_ENCODER, got: {err}"
    );
    assert_eq!(*state.stream_state.lock().unwrap(), StreamState::Idle);
    assert!(state.stream_session.lock().unwrap().is_none());
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
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
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
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
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
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
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
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
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
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
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
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
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
async fn stream_tap_path_cpu_readback_is_refused_no_zerocopy() {
    // Recording's v0.4.2 readback allowance does not extend to streaming
    // (§0.1 assumption 24): a `cpuReadback` stream take has no lawful path,
    // so `stream.start` refuses on the E_NO_ZEROCOPY path — never an
    // Override-live selection. Hardware-free: the refusal precedes the
    // encoder and chain probes.
    let _serial = SERIAL.lock().await;
    let (state, handler, outgoing) = harness();
    let (_pkg, pkg_path) = write_package(
        Some("rtmp://manifest.example/live/key"),
        Some("cpuReadback"),
    );
    load_and_start(&handler, &pkg_path).await;

    let err = handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect_err("a cpuReadback stream take must be refused");

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
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
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
            serde_json::json!({"url": "rtmp://other.example/live/key"}),
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
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// 7. `url` validation: garbage-scheme and non-string urls refuse E_BAD_PAYLOAD
// (hardware-free — the endpoint check precedes the probes); an uppercase
// RTMP:// scheme resolves (case-insensitive match, parsed by rtmp.rs).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_start_with_garbage_url_refuses_bad_payload() {
    // Garbage must never go Live with publisher=None: the endpoint is PARSED
    // at resolve time (the publisher's own parser since PR #33's fix round;
    // ~~"the scheme is validated"~~ let a keyless URL through, §2c), not
    // discovered later by a missing transport.
    let _serial = SERIAL.lock().await;
    let (state, handler, outgoing) = harness();
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
    load_and_start(&handler, &pkg_path).await;

    let err = handler
        .apply(&directive(
            "stream.start",
            3,
            serde_json::json!({"url": "notaurl"}),
        ))
        .await
        .expect_err("a non-rtmp url must refuse");

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
async fn stream_start_with_non_string_url_refuses_bad_payload() {
    // A present-but-non-string `url` is a malformed endpoint, never silent:
    // silently falling back to the manifest would publish somewhere the
    // operator did not name.
    let _serial = SERIAL.lock().await;
    let (state, handler, outgoing) = harness();
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
    load_and_start(&handler, &pkg_path).await;

    let err = handler
        .apply(&directive(
            "stream.start",
            3,
            serde_json::json!({"url": 42}),
        ))
        .await
        .expect_err("a non-string url must refuse");

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
async fn stream_start_with_uppercase_scheme_resolves() {
    // Case-insensitive `rtmp://` match at resolve time (parsed by rtmp.rs):
    // the endpoint is kept verbatim and the stream opens.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
    load_and_start(&handler, &pkg_path).await;

    handler
        .apply(&directive(
            "stream.start",
            3,
            serde_json::json!({"url": "RTMP://127.0.0.1:9/live/key"}),
        ))
        .await
        .expect("an uppercase RTMP:// url must resolve and open");
    assert_live(&state);
    assert_eq!(
        endpoint_of(&state),
        "RTMP://127.0.0.1:9/live/key",
        "the resolved endpoint is kept verbatim"
    );

    // Settle: let a failing transport's first dial complete so its publisher
    // has a live-loop bunker before teardown (the immediate-stop case races
    // the dial: shutdown then time-outs while the task is still connecting on
    // a listenerless port).
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
}

// ---------------------------------------------------------------------------
// 8. Fresh counters per stream + selection cleared on every stop path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_counters_reset_per_stream_and_selection_clears_on_stop() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, _) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    // Dirty the stream counters through the loop's own handoff (no state
    // writes): three surfaces offered to a channel with no room are three
    // stream drops on the live stream's counter.
    {
        let guard = state.stream_session.lock().unwrap();
        let sess = guard.as_ref().expect("stream session must be live");
        let surface = sess
            .surface_pool()
            .expect("a live zero-copy stream owns a pool")
            .acquire()
            .expect("a fresh pool has a free surface");
        let (full, _held) = std::sync::mpsc::sync_channel(0);
        for frame in 0..3 {
            assert!(!stream_glue::hand_off_stream_surface(
                &full,
                surface.clone(),
                frame,
                &state.skipped_stream_frames,
            ));
        }
    }
    assert_eq!(
        state
            .skipped_stream_frames
            .load(std::sync::atomic::Ordering::SeqCst),
        3,
        "three dropped frames must be counted"
    );

    handler
        .apply(&directive("stream.stop", 4, serde_json::json!({})))
        .await
        .expect("stop must succeed");
    assert!(
        state.stream_tap_selection.lock().unwrap().is_none(),
        "a stopped stream publishes no selection"
    );

    handler
        .apply(&directive("stream.start", 5, serde_json::json!({})))
        .await
        .expect("second stream.start must open");
    assert_eq!(
        state
            .skipped_stream_frames
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a new stream starts its drop counter from zero (record-start shape)"
    );
    assert_eq!(
        *state.stream_tap_ms.lock().unwrap(),
        0.0,
        "a new stream starts its tap-ms counter from zero"
    );
    assert!(
        state.stream_tap_selection.lock().unwrap().is_some(),
        "a new stream publishes a fresh selection"
    );

    handler
        .apply(&directive("stream.stop", 6, serde_json::json!({})))
        .await
        .expect("cleanup stop must succeed");
    assert!(
        state.stream_tap_selection.lock().unwrap().is_none(),
        "stop clears the selection again"
    );
}

// ---------------------------------------------------------------------------
// 9. `show.stop` with both outputs live: concurrent teardowns stay inside the
// §16.1 2 s window with first-failure-wins; a failing stream alone withholds
// the ack through the same concurrent path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn show_stop_with_both_live_quiesces_inside_two_seconds_first_failure_wins() {
    // Both outputs live (record take empty, stream transport-less): the record
    // quiesce fails E_RECORD_INPUT (nothing to finalize) while the stream
    // closes cleanly — the record error wins (ordered first), the ack is
    // withheld, both outputs are consumed, and the whole stop fits the window
    // the sequential worst case (1500 ms + 800 ms) would exceed.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path, _rec) =
        write_record_package(Some("rtmp://manifest.example/live/key"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("record.start", 3, serde_json::json!({})))
        .await
        .expect("record.start must open");
    handler
        .apply(&directive("stream.start", 4, serde_json::json!({})))
        .await
        .expect("stream.start must open alongside the take");

    let start = Instant::now();
    let err = handler
        .apply(&directive("show.stop", 5, serde_json::json!({})))
        .await
        .expect_err("an empty take cannot finalize: the record error must surface");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "both-live quiesce must fit the §16.1 2 s window, took {elapsed:?}"
    );
    assert!(
        err.to_string().contains("E_RECORD_INPUT"),
        "first failure wins (record ordered first), got: {err}"
    );
    assert_ne!(
        state.last_applied(),
        5,
        "failed quiesce must not advance applied"
    );
    assert!(!acked(&outgoing, 5), "failed quiesce must not ack");
    assert_eq!(
        *state.record_state.lock().unwrap(),
        nbe_engine::state::RecordState::Idle,
        "record quiesce consumes the take even on failure"
    );
    assert!(state.record_session.lock().unwrap().is_none());
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "stream quiesce runs beside the failing record"
    );
    assert!(state.stream_session.lock().unwrap().is_none());
    assert!(
        state.stream_tap_selection.lock().unwrap().is_none(),
        "quiesced stream publishes no selection"
    );
    assert!(
        !state.is_running(),
        "failed quiesce still stops the show clock"
    );
}

#[tokio::test]
async fn show_stop_with_failing_stream_withholds_ack() {
    // Stream-only failure through the concurrent path: the teardown error
    // surfaces (no ack, applied frozen) and the live state still ends.
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    let _force = ForceCloseErrorGuard::set();
    let start = Instant::now();
    let err = handler
        .apply(&directive("show.stop", 4, serde_json::json!({})))
        .await
        .expect_err("a failed stream teardown must surface, never ack");
    let elapsed = start.elapsed();

    assert!(
        err.to_string().contains("E_NETWORK"),
        "expected E_NETWORK, got: {err}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "failed quiesce must fit the §16.1 2 s window, took {elapsed:?}"
    );
    assert_ne!(
        state.last_applied(),
        4,
        "failed quiesce must not advance applied"
    );
    assert!(!acked(&outgoing, 4), "failed quiesce must not ack");
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "failed teardown still ends the live state"
    );
    assert!(state.stream_session.lock().unwrap().is_none());
}

// ---------------------------------------------------------------------------
// 10. A `stream.stop` racing the feed leg's session lock returns boundedly:
// session-before-state order means wait, never deadlock. The holder is a real
// OS thread (the feed leg holds the guard only across bounded `try_send`s);
// the stop must proceed once it releases.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_stop_during_feed_lock_returns_boundedly() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let (state, handler, outgoing) = harness();
    if !chain_or_skip(&state).await {
        return;
    }
    let (_pkg, pkg_path) = write_package(Some("rtmp://manifest.example/live/key"), None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive("stream.start", 3, serde_json::json!({})))
        .await
        .expect("stream.start must open");
    assert_live(&state);

    let feed_state = state.clone();
    let (taken_tx, taken_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _held = feed_state.stream_session.lock().unwrap();
        // Signal UNDER the guard, so the stop below provably waits on it
        // rather than winning the race uncontended.
        let _ = taken_tx.send(());
        std::thread::sleep(Duration::from_millis(300));
    });
    taken_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("holder must take the feed lock");
    let start = Instant::now();
    tokio::time::timeout(
        Duration::from_secs(5),
        handler.apply(&directive("stream.stop", 4, serde_json::json!({}))),
    )
    .await
    .expect("stream.stop must return boundedly, never deadlock")
    .expect("stop after the feed lock releases must close cleanly");
    let elapsed = start.elapsed();
    holder.join().expect("holder thread must exit");

    assert!(
        elapsed < Duration::from_secs(5),
        "stop-during-feed took {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(200),
        "stop must have waited on the feed lock, took {elapsed:?}"
    );
    assert_eq!(
        *state.stream_state.lock().unwrap(),
        StreamState::Idle,
        "stop must return the state machine to Idle"
    );
    assert!(state.stream_session.lock().unwrap().is_none());
    assert!(acked(&outgoing, 4));
}
