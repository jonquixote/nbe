//! Integration tests for nbe-preflight (SPEC Section 19).
//! Exit codes: 0 air-ready, 1 warnings only, 2 errors.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_nbe-preflight");

/// Result of one preflight run.
struct Run {
    code: i32,
    stderr: String,
}

fn run_preflight(pkg: &Path, args: &[&str]) -> Run {
    let out = Command::new(BIN)
        .arg("--package-path")
        .arg(pkg)
        .args(args)
        .output()
        .expect("failed to run nbe-preflight");
    Run {
        code: out.status.code().expect("preflight returned no exit code"),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn read_report(pkg: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(pkg.join("preflight_report.json"))
        .expect("preflight must write a report on every run (P1-D3)");
    serde_json::from_str(&raw).expect("report is valid JSON")
}

/// Path to a generated media fixture (see scripts/generate-fixtures.sh).
fn media_fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/media")
        .join(name)
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

    let output = Command::new(BIN)
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

    let output = Command::new(BIN)
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

    let output = Command::new(BIN)
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

    let report_text = std::fs::read_to_string(pkg.join("preflight_report.json")).unwrap();
    let report: serde_json::Value = serde_json::from_str(&report_text).unwrap();
    assert_eq!(report["manifestValid"], true);
    assert_eq!(report["airReady"], true);
    assert_eq!(report["errors"], serde_json::json!([]));
}

/// The exit-1 / `--allow-warnings` path, live at last.
///
/// Prompt 01 wired this path and left this test ignored, because nothing
/// called `push_warning`. Prompt 05's decode checks are the first warning
/// producer: a clip declaring a different `expectedDurationFrames` than it
/// actually decodes is worth an operator's attention but does not stop the
/// show.
#[test]
fn warnings_only_exits_1_without_flag_0_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path();
    std::fs::create_dir_all(pkg.join("media")).unwrap();
    // A real 640x360 30 fps clip of 30 frames, declared as 999.
    std::fs::copy(media_fixture("cfr_30.mp4"), pkg.join("media/clip.mp4")).unwrap();
    std::fs::write(pkg.join("media/slate.png"), b"placeholder").unwrap();
    std::fs::write(
        pkg.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "Warnings" },
            "show": {
                "id": "warn", "title": "Warnings only",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" },
                { "id": "clip", "kind": "video", "source": "media/clip.mp4", "format": "h264",
                  "expectedDurationFrames": 999 }
            ],
            "scenes": [{ "id": "SCN", "elements": [
                { "id": "main", "kind": "clip", "z": 1, "assetId": "clip" }
            ]}],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();

    // Without the flag: exit 1, warnings present, no errors, not air-ready.
    let out = run_preflight(pkg, &[]);
    assert_eq!(out.code, 1, "warnings-only must exit 1: {}", out.stderr);
    let report = read_report(pkg);
    assert_eq!(report["errors"], serde_json::json!([]));
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("duration")),
        "expected a duration warning, got {:?}",
        report["warnings"]
    );
    assert_eq!(report["airReady"], false);

    // With the flag: exit 0, and the warnings are still recorded.
    let out = run_preflight(pkg, &["--allow-warnings"]);
    assert_eq!(out.code, 0, "--allow-warnings must exit 0: {}", out.stderr);
    let report = read_report(pkg);
    assert!(!report["warnings"].as_array().unwrap().is_empty());
}

/// The decode-based seeded failures (SPEC §19.3, AC-3).
#[test]
fn vfr_and_wrong_resolution_fixtures_exit_2_with_reasons() {
    for (media, needle) in [("vfr.mp4", "vfr:"), ("wrong_res.mp4", "resolution:")] {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path();
        std::fs::create_dir_all(pkg.join("media")).unwrap();
        std::fs::copy(media_fixture(media), pkg.join("media/clip.mp4")).unwrap();
        std::fs::write(pkg.join("media/slate.png"), b"placeholder").unwrap();
        std::fs::write(
            pkg.join("manifest.json"),
            serde_json::json!({
                "manifestVersion": "0.3",
                "network": { "id": "nbe", "name": "Seeded" },
                "show": {
                    "id": "seed", "title": "Seeded failure",
                    "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                    "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                    "fallbackAssetId": "slate"
                },
                "assets": [
                    { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" },
                    { "id": "clip", "kind": "video", "source": "media/clip.mp4", "format": "h264" }
                ],
                "scenes": [{ "id": "SCN", "elements": [
                    { "id": "main", "kind": "clip", "z": 1, "assetId": "clip" }
                ]}],
                "rundown": { "id": "R", "items": [
                    { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }
                ]},
                "control": { "bindings": [] }
            })
            .to_string(),
        )
        .unwrap();

        let out = run_preflight(pkg, &[]);
        assert_eq!(out.code, 2, "{media} must exit exactly 2: {}", out.stderr);
        let report = read_report(pkg);
        let errors = report["errors"].as_array().unwrap();
        assert!(
            errors
                .iter()
                .any(|e| e.as_str().unwrap().starts_with(needle)),
            "{media}: expected an error starting {needle:?}, got {errors:?}"
        );
        assert_eq!(report["airReady"], false);
    }
}

/// A file that is not decodable video is an error, not a silent pass.
#[test]
fn a_corrupt_asset_exits_2_naming_decode() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path();
    std::fs::create_dir_all(pkg.join("media")).unwrap();
    std::fs::copy(media_fixture("corrupt.mp4"), pkg.join("media/clip.mp4")).unwrap();
    std::fs::write(pkg.join("media/slate.png"), b"placeholder").unwrap();
    std::fs::write(
        pkg.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "Corrupt" },
            "show": {
                "id": "corrupt", "title": "Corrupt",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" },
                { "id": "clip", "kind": "video", "source": "media/clip.mp4", "format": "h264" }
            ],
            "scenes": [{ "id": "SCN", "elements": [] }],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();

    let out = run_preflight(pkg, &[]);
    assert_eq!(out.code, 2, "a corrupt asset must exit 2: {}", out.stderr);
    let report = read_report(pkg);
    assert!(report["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e.as_str().unwrap().starts_with("decode:")));
}
