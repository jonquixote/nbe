//! Prompt 08 work item 5: control.bindings name registered §16 commands
//! and carry well-formed triggers. Exit codes: 0 air-ready, 2 errors.

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

fn write(dir: &Path, bindings: serde_json::Value) {
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
        "overlays": [],
        "rundown": { "id": "R", "items": [ { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" } ] },
        "control": { "bindings": bindings }
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
fn valid_bindings_pass() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        serde_json::json!([
            { "id": "b-take", "action": "view.take", "payload": {},
              "trigger": { "kind": "hotkey", "key": "t" } },
            { "id": "b-cut", "action": "view.cut", "payload": { "itemRef": "A1" },
              "trigger": { "kind": "companionKey", "page": 1, "bank": 1, "key": "1-1-1" } },
            { "id": "b-overlay", "action": "overlay.show", "payload": { "overlayId": "bug" } }
        ]),
    );
    let (code, report) = run(tmp.path());
    assert_eq!(code, 0, "report: {report}");
    assert_eq!(report["airReady"], true);
}

#[test]
fn unknown_action_and_bad_trigger_fail() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        serde_json::json!([
            { "id": "b-bogus", "action": "nope.doesNotExist",
              "trigger": { "kind": "hotkey" } },
            { "id": "b-cut-bad", "action": "view.cut", "payload": {} }
        ]),
    );
    let (code, report) = run(tmp.path());
    assert_eq!(code, 2);
    assert_eq!(report["airReady"], false);
    let errors = report_errors(&report);
    assert!(
        errors.iter().any(|e| e.contains("b-bogus")
            && e.contains("invalidBinding")
            && e.contains("E_PREFLIGHT_FAILED")),
        "errors: {errors:?}"
    );
    assert!(
        errors.iter().any(|e| e.contains("b-bogus")
            && e.contains("invalidBindingTrigger")
            && e.contains("E_PREFLIGHT_FAILED")),
        "errors: {errors:?}"
    );
    assert!(
        errors.iter().any(|e| e.contains("b-cut-bad")
            && e.contains("itemRef")
            && e.contains("E_PREFLIGHT_FAILED")),
        "errors: {errors:?}"
    );
}
