//! Step 5 §3.4: overlays — unique IDs and resolvable element references.
//! Exit codes: 0 air-ready, 2 errors.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_nbe-preflight");

fn run(pkg: &Path) -> (i32, serde_json::Value) {
    let out = Command::new(BIN)
        .arg("--package-path")
        .arg(pkg)
        .output()
        .expect("run preflight");
    let code = out.status.code().expect("exit code");
    let report = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(pkg.join("preflight_report.json")).expect("report written"),
    )
    .expect("report parses");
    (code, report)
}

fn write(dir: &Path, overlays: serde_json::Value) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    std::fs::write(dir.join("media/slate.png"), "png").unwrap();
    let manifest = serde_json::json!({
        "manifestVersion": "0.3",
        "network": { "id": "n", "name": "T" },
        "show": {
            "id": "s", "title": "T",
            "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
            "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
            "fallbackAssetId": "slate"
        },
        "assets": [ { "id": "slate", "kind": "image", "source": "media/slate.png" } ],
        "scenes": [ { "id": "SCN", "elements": [] } ],
        "templates": [ { "id": "tpl", "kind": "generic" } ],
        "overlays": overlays,
        "rundown": { "id": "R", "items": [ { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" } ] },
        "control": { "bindings": [] }
    });
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

fn report_errors(report: &serde_json::Value) -> Vec<String> {
    report["errors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn valid_overlay_fixture_passes() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        serde_json::json!([
            { "id": "bug", "elements": [
                { "id": "b1", "kind": "graphic", "z": 1, "templateId": "tpl" } ] }
        ]),
    );
    let (code, report) = run(tmp.path());
    assert_eq!(code, 0, "report: {report}");
    assert_eq!(report["airReady"], true);
}

#[test]
fn duplicate_overlay_ids_fail() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        serde_json::json!([
            { "id": "bug", "elements": [ { "id": "b1", "kind": "graphic", "z": 1, "templateId": "tpl" } ] },
            { "id": "bug", "elements": [ { "id": "b2", "kind": "graphic", "z": 2, "templateId": "tpl" } ] }
        ]),
    );
    let (code, report) = run(tmp.path());
    assert_eq!(code, 2);
    assert!(
        report_errors(&report)
            .iter()
            .any(|e| e.contains("duplicateOverlay")),
        "errors: {:?}",
        report_errors(&report)
    );
}

#[test]
fn overlay_element_with_missing_asset_reference_fails() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        serde_json::json!([
            { "id": "bug", "elements": [
                { "id": "b1", "kind": "clip", "z": 1, "assetId": "nope" } ] }
        ]),
    );
    let (code, report) = run(tmp.path());
    assert_eq!(code, 2);
    assert!(
        report_errors(&report)
            .iter()
            .any(|e| e.contains("overlayAsset")),
        "errors: {:?}",
        report_errors(&report)
    );
}

#[test]
fn overlay_element_with_missing_template_reference_fails() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        serde_json::json!([
            { "id": "bug", "elements": [
                { "id": "b1", "kind": "ticker", "z": 1, "templateId": "no-such-template" } ] }
        ]),
    );
    let (code, report) = run(tmp.path());
    assert_eq!(code, 2);
    assert!(
        report_errors(&report)
            .iter()
            .any(|e| e.contains("overlayTemplate")),
        "errors: {:?}",
        report_errors(&report)
    );
}
