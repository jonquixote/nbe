//! WU2 — tapPath override wiring: `outputs.record.tapPath` reaches the take.
//!
//! The ledger debt (`docs/09-measurements.md`, "On the ledger"): v0.4.5 gave
//! `outputs.{record,stream}.tapPath: { auto, cpuReadback }` (default `auto`),
//! and `select_with_override` already existed tested — with zero callers. A
//! manifest pinning `cpuReadback` still ran the table's path.
//!
//! `"auto"` = the table speaks (default, product decision). `"cpuReadback"` =
//! restrict: force CPU, never conjure — a cpuReadback take on a
//! zero-copy-capable machine runs CPU and reports reason `Override`.
//!
//! Every take test below drives the real `record.start` path (show.load with a
//! real package → show.start → record.start) and reads the selection out of
//! engine state + a real `build_tick` — never a hand-written selection.

use std::sync::Arc;

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::record::tap_path::{select, select_stream, Consumer, Reason, TapPath};
use nbe_engine::render::{RenderLoop, VIEW_H};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};

/// One take at a time: the hardware encoder is a shared resource.
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

fn hw_or_skip() -> bool {
    if nbe_engine::record::encoder_available() {
        return true;
    }
    eprintln!("SKIP: no hardware H.264 encoder on this machine; record.start cannot open a take");
    false
}

/// A minimal loadable package whose record output carries `directory` and,
/// when given, `tapPath`. Returns the package dir (for show.load) — the
/// caller keeps the record-dir TempDir alive.
fn write_package(tap: Option<&str>) -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let pkg = tempfile::tempdir().expect("package tempdir must succeed");
    let rec = tempfile::tempdir().expect("record tempdir must succeed");
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
    let mut record = serde_json::json!({ "directory": rec.path().to_string_lossy() });
    if let Some(t) = tap {
        record["tapPath"] = t.into();
    }
    std::fs::write(
        pkg.path().join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate",
                "outputs": { "record": record }
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
    (pkg, rec, pkg_path)
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

/// The path + reason telemetry reports, read out of a REAL `build_tick`.
fn reported(state: &Arc<EngineState>) -> (String, String) {
    let frame = nbe_engine::telemetry::build_tick(state);
    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } = frame else {
        panic!("build_tick must produce telemetry");
    };
    (fields.record_tap_path, fields.record_tap_reason)
}

fn selection_of(state: &Arc<EngineState>) -> nbe_engine::record::tap_path::Selection {
    state
        .record_tap_selection
        .lock()
        .unwrap()
        .expect("a started take must have published its selection")
}

// ---------------------------------------------------------------------------
// 1. Flagship — a cpuReadback manifest restricts the take and says Override.
// BEFORE the wiring this runs the table's path (zeroCopy/Table on a capable
// machine, cpuReadback/ProbeUnavailable without one): RED.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cpu_readback_manifest_restricts_the_take_to_cpu_and_reports_override() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    // A capable machine is the case that matters (restrict, never conjure):
    // publish the device where there is one; where there is none the take is
    // incapable and the assertions below still hold (CPU + Override).
    let _render = RenderLoop::new(state.clone()).await.ok();

    let (_pkg, _rec, pkg_path) = write_package(Some("cpuReadback"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive(
            "record.start",
            3,
            serde_json::json!({"outputId": "ep01"}),
        ))
        .await
        .expect("record.start on a RUNNING show must open the pipeline");

    let sel = selection_of(&state);
    assert_eq!(
        sel.path,
        TapPath::CpuReadback,
        "a cpuReadback take runs CPU even where the chain is available"
    );
    assert_eq!(
        sel.reason,
        Reason::Override,
        "the restriction must say Override, not Table: an operator action, not the table's choice"
    );
    assert_eq!(
        reported(&state),
        ("cpuReadback".into(), "Override".into()),
        "the restriction must reach the wire — the operator asked for this path"
    );
    assert!(
        state
            .record_session
            .lock()
            .unwrap()
            .as_ref()
            .expect("session")
            .surface_pool()
            .is_none(),
        "a restricted take builds no surface pool: the chain it must not use costs ~25 MiB of VRAM"
    );

    // Empty take: loud at finish, state back to Idle either way.
    let _ = handler
        .apply(&directive("record.stop", 4, serde_json::json!({})))
        .await;
    nbe_engine::record::markers::clear();
}

// ---------------------------------------------------------------------------
// 2. Absent tapPath pins existing behavior: the table speaks, never Override.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn absent_tap_path_keeps_the_table() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    let _render = RenderLoop::new(state.clone()).await.ok();

    let (_pkg, _rec, pkg_path) = write_package(None);
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive(
            "record.start",
            3,
            serde_json::json!({"outputId": "ep01"}),
        ))
        .await
        .expect("record.start on a RUNNING show must open the pipeline");

    let capable = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .expect("session")
        .surface_pool()
        .is_some();
    let expected = select(capable, VIEW_H, Consumer::Record);
    let sel = selection_of(&state);
    assert_eq!(
        sel, expected,
        "with no tapPath the published table is the only voice"
    );
    assert_ne!(
        sel.reason,
        Reason::Override,
        "nothing was overridden: the reason must stay the table's (or the probe's)"
    );
    assert_eq!(
        reported(&state),
        (
            expected.path.as_str().to_string(),
            format!("{:?}", expected.reason)
        ),
        "telemetry must report the table's answer, unchanged"
    );

    let _ = handler
        .apply(&directive("record.stop", 4, serde_json::json!({})))
        .await;
    nbe_engine::record::markers::clear();
}

// ---------------------------------------------------------------------------
// 3. Restrict-only: cpuReadback on an incapable machine is still CPU —
//    the override never conjures a capability the probe denied.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cpu_readback_on_an_incapable_machine_stays_cpu() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    let _render = RenderLoop::new(state.clone()).await.ok();
    // The incapable precondition, the migration suite's own seam: no device
    // is a machine with no chain.
    *state.render_device.lock().unwrap() = None;

    let (_pkg, _rec, pkg_path) = write_package(Some("cpuReadback"));
    load_and_start(&handler, &pkg_path).await;
    handler
        .apply(&directive(
            "record.start",
            3,
            serde_json::json!({"outputId": "ep01"}),
        ))
        .await
        .expect("record.start on a RUNNING show must open the pipeline");

    let sel = selection_of(&state);
    assert_eq!(
        sel.path,
        TapPath::CpuReadback,
        "an override to CPU on an incapable machine is still CPU — nothing was conjured"
    );

    let _ = handler
        .apply(&directive("record.stop", 4, serde_json::json!({})))
        .await;
    nbe_engine::record::markers::clear();
}

// ---------------------------------------------------------------------------
// 4. No behavior change to the refusal paths (hardware-free).
// ---------------------------------------------------------------------------

#[test]
fn refusal_paths_are_unchanged() {
    // Record on an incapable machine stays a lawful take under the allowance.
    let s = select(false, VIEW_H, Consumer::Record);
    assert_eq!(s.path, TapPath::CpuReadback);
    assert_eq!(s.reason, Reason::ProbeUnavailable);
    // Streaming has no lawful path without the chain — the v0.4.2 allowance
    // is recording-only — and an override cannot conjure one either.
    assert!(select_stream(false).is_none());
    assert_eq!(
        select_stream(true).expect("capable stream").path,
        TapPath::ZeroCopy
    );
    let conjure = nbe_engine::record::tap_path::select_with_override(
        false,
        VIEW_H,
        Consumer::Record,
        Some(TapPath::ZeroCopy),
    );
    assert_eq!(conjure.path, TapPath::CpuReadback);
    assert_eq!(conjure.reason, Reason::ProbeUnavailable);
}

// ---------------------------------------------------------------------------
// 5. The field is law in the typed model (hardware-free): no schema edits.
// ---------------------------------------------------------------------------

#[test]
fn tap_path_field_is_law_in_the_typed_model() {
    let manifest: nbe_core::manifest::Manifest = serde_json::from_value(serde_json::json!({
        "manifestVersion": "0.4",
        "network": { "id": "nbe", "name": "T" },
        "show": {
            "id": "s", "title": "T",
            "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
            "audio": { "sampleRate": 48000 },
            "fallbackAssetId": "slate",
            "outputs": { "record": { "directory": "./out", "tapPath": "cpuReadback" } }
        },
        "assets": [],
        "scenes": [],
        "rundown": { "id": "R", "items": [] },
        "control": { "bindings": [] }
    }))
    .expect("the v0.4.5 field must parse in the typed model");
    let record = manifest
        .show
        .outputs
        .as_ref()
        .and_then(|o| o.record.as_ref())
        .expect("outputs.record must survive parsing");
    assert_eq!(
        record.tap_path,
        nbe_core::manifest::TapPathPreference::CpuReadback
    );

    // Absent → auto: the table speaks by default.
    let manifest: nbe_core::manifest::Manifest = serde_json::from_value(serde_json::json!({
        "manifestVersion": "0.4",
        "network": { "id": "nbe", "name": "T" },
        "show": {
            "id": "s", "title": "T",
            "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
            "audio": { "sampleRate": 48000 },
            "fallbackAssetId": "slate",
            "outputs": { "record": { "directory": "./out" } }
        },
        "assets": [],
        "scenes": [],
        "rundown": { "id": "R", "items": [] },
        "control": { "bindings": [] }
    }))
    .expect("a manifest without tapPath must still parse");
    let record = manifest
        .show
        .outputs
        .as_ref()
        .and_then(|o| o.record.as_ref())
        .expect("outputs.record must survive parsing");
    assert_eq!(record.tap_path, nbe_core::manifest::TapPathPreference::Auto);
}
