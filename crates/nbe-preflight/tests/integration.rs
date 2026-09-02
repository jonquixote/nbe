//! Integration test: a package whose manifest references a missing asset
//! must exit 2 (SPEC Section 19.1, seeded failure #1 in Section 19.3).

use std::path::{Path, PathBuf};
use std::process::Command;

fn cargo_bin() -> PathBuf {
    // Built by cargo; tests run from the crate dir.
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target_dir.join("debug").join("nbe-preflight")
}

fn write_manifest(dir: &Path, manifest: serde_json::Value) {
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

fn minimal_manifest_with_asset(asset_id: &str, source: &str) -> serde_json::Value {
    serde_json::json!({
        "manifestVersion": "0.3",
        "network": { "id": "nbe", "name": "Test" },
        "show": {
            "id": "show",
            "title": "Test Show",
            "video": { "width": 1920, "height": 1080, "frameRate": 30, "colorSpace": "rec709" },
            "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16, "truePeakDbtp": -1.5 },
            "fallbackAssetId": asset_id
        },
        "assets": [
            { "id": asset_id, "kind": "image", "source": source }
        ],
        "scenes": [
            { "id": "SCN_A", "elements": [] }
        ],
        "rundown": {
            "id": "R",
            "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_A" }
            ]
        },
        "control": { "bindings": [] }
    })
}

#[test]
fn missing_asset_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path();
    // Manifest references an asset that does not exist on disk.
    write_manifest(
        pkg,
        minimal_manifest_with_asset("fallback", "media/fallback.png"),
    );
    // Deliberately do NOT create media/fallback.png.

    let bin = cargo_bin();
    let output = Command::new(bin)
        .arg("--package-path")
        .arg(pkg)
        .output()
        .expect("failed to run nbe-preflight");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2 for missing asset; stdout: {} stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Report must be machine-readable and record the missing asset.
    let report_text = std::fs::read_to_string(pkg.join("preflight_report.json")).unwrap();
    let report: serde_json::Value = serde_json::from_str(&report_text).unwrap();
    assert_eq!(report["manifestValid"], true);
    assert_eq!(report["airReady"], false);
    assert!(
        report["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap().contains("missing asset")),
        "expected a missing-asset error in report: {report_text}"
    );
}

#[test]
fn v02_manifest_exits_2_with_migration_guidance() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path();
    let mut manifest = minimal_manifest_with_asset("fallback", "media/fallback.png");
    manifest["manifestVersion"] = serde_json::json!("0.2");
    write_manifest(pkg, manifest);
    // v0.2 manifest: version gate fires before asset checks; a missing
    // asset would just be noise. Create it so the report is clean.
    std::fs::create_dir_all(pkg.join("media")).unwrap();
    std::fs::write(pkg.join("media/fallback.png"), b"not-a-real-png").unwrap();

    let bin = cargo_bin();
    let output = Command::new(bin)
        .arg("--package-path")
        .arg(pkg)
        .output()
        .expect("failed to run nbe-preflight");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2 for v0.2 manifest; stdout: {} stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report_text = std::fs::read_to_string(pkg.join("preflight_report.json")).unwrap();
    assert!(
        report_text.contains("migrationRequired") && report_text.contains("nbe-migrate"),
        "expected migration guidance in report: {report_text}"
    );
}

#[test]
fn valid_package_exits_0() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path();
    write_manifest(
        pkg,
        minimal_manifest_with_asset("fallback", "media/fallback.png"),
    );
    std::fs::create_dir_all(pkg.join("media")).unwrap();
    std::fs::write(pkg.join("media/fallback.png"), b"not-a-real-png").unwrap();

    let bin = cargo_bin();
    let output = Command::new(bin)
        .arg("--package-path")
        .arg(pkg)
        .output()
        .expect("failed to run nbe-preflight");

    assert_eq!(
        output.status.code(),
        Some(0),
        "expected exit 0; stdout: {} stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
