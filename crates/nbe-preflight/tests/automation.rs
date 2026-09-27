//! Prompt 11: preflight's automation checks (Standards §2a: each fails when
//! removed). B1: an `audioLevel` rule whose params do not read is refused by
//! name — a rule that can never fire is not accepted and left inert.

use std::path::Path;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_nbe-preflight")
}

/// A minimal schema-valid package with `automation` spliced in verbatim.
fn package(dir: &Path, automation: &str) -> std::path::PathBuf {
    let root = dir.join("pkg");
    std::fs::create_dir_all(root.join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(root.join("media/slate.png"), &png).unwrap();
    let manifest = format!(
        r#"{{
      "manifestVersion": "0.4",
      "network": {{ "id": "nbe", "name": "T" }},
      "show": {{
        "id": "s", "title": "T",
        "video": {{ "width": 1920, "height": 1080, "frameRate": 30, "colorSpace": "rec709" }},
        "audio": {{ "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 }},
        "fallbackAssetId": "slate"
      }},
      "control": {{ "bindings": [] }},
      "assets": [{{ "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" }}],
      "scenes": [{{ "id": "SCN", "elements": [
        {{ "id": "bg", "kind": "graphic", "z": 0, "templateId": "TPL" }}
      ] }}],
      "templates": [{{ "id": "TPL", "kind": "generic" }}],
      "rundown": {{ "id": "R", "items": [{{ "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }}] }},
      "automation": [{automation}]
    }}"#
    );
    std::fs::write(root.join("manifest.json"), manifest).unwrap();
    root
}

fn run(root: &Path) -> (i32, serde_json::Value) {
    let out = Command::new(bin())
        .arg("--package-path")
        .arg(root)
        .output()
        .expect("preflight runs");
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("preflight_report.json")).expect("report written"),
    )
    .expect("report is JSON");
    (out.status.code().unwrap_or(-1), report)
}

fn errors(report: &serde_json::Value) -> Vec<String> {
    report["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn a_malformed_audio_level_rule_fails_preflight_by_name() {
    let dir = tempfile::tempdir().unwrap();
    for (params, needle) in [
        (
            r#"{ "thresholdDbfs": -12 }"#,
            "needs a non-empty string `bus`",
        ),
        (
            r#"{ "bus": "program", "thresholdDbfs": -12 }"#,
            "does not meter",
        ),
        (
            r#"{ "bus": "mic", "treshold": -12 }"#,
            "unknown param `treshold`",
        ),
        (r#"{ "bus": "mic", "thresholdDbfs": 6 }"#, "outside"),
    ] {
        let root = package(
            dir.path(),
            &format!(
                r#"{{ "id": "hot", "trigger": {{ "kind": "audioLevel", "params": {params} }},
                     "action": {{ "command": "marker.add", "payload": {{ "name": "m" }} }} }}"#
            ),
        );
        let (code, report) = run(&root);
        let errs = errors(&report);
        assert_eq!(
            code, 2,
            "{params} must fail preflight, not warn; errors {errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("automationRule") && e.contains("`hot`") && e.contains(needle)),
            "the error must name the rule and the reason ({needle}); got {errs:?}"
        );
    }
}

#[test]
fn a_complete_audio_level_rule_passes_preflight() {
    let dir = tempfile::tempdir().unwrap();
    let root = package(
        dir.path(),
        r#"{ "id": "hot", "trigger": { "kind": "audioLevel",
                "params": { "bus": "guest:g1", "thresholdDbfs": -12, "direction": "falling" } },
             "action": { "command": "marker.add", "payload": { "name": "m" } } }"#,
    );
    let (code, report) = run(&root);
    let errs = errors(&report);
    assert!(
        errs.iter().all(|e| !e.contains("automationRule")),
        "a complete rule is not refused; errors {errs:?}"
    );
    assert_eq!(code, 0, "and the package is air-ready; report {report}");
}
