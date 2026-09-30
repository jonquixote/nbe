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

#[test]
fn every_trigger_kind_is_read_and_a_rule_that_cannot_fire_fails_by_name() {
    // WU1: the whole params contract, not only audioLevel. The verdict for
    // each shape is shared with the control plane's reading
    // (nbe-core/tests/fixtures/automation_rules.json); this checks preflight
    // REFUSES by it, with the rule named.
    let dir = tempfile::tempdir().unwrap();
    for (rule, needle) in [
        (
            r#"{ "id": "rss", "trigger": { "kind": "rssKeyword", "params": { "keyword": "x" } },
                 "action": { "command": "marker.add", "payload": { "name": "m" } } }"#,
            "rule `rss`: rssKeyword trigger has no source",
        ),
        (
            r#"{ "id": "hk", "trigger": { "kind": "hotkey", "params": { "bindingId": "nope" } },
                 "action": { "command": "marker.add", "payload": { "name": "m" } } }"#,
            "rule `hk`: hotkey trigger bindingId",
        ),
        (
            r#"{ "id": "sc", "trigger": { "kind": "stateChange", "params": { "field": "tally" } },
                 "action": { "command": "marker.add", "payload": { "name": "m" } } }"#,
            "rule `sc`: stateChange trigger field",
        ),
        (
            r#"{ "id": "cmd", "trigger": { "kind": "mediaEnd" },
                 "action": { "command": "view.warp" } }"#,
            "rule `cmd`: action command `view.warp` is not a command",
        ),
        (
            r#"{ "id": "key", "trigger": { "kind": "mediaEnd" },
                 "action": { "command": "view.cut" } }"#,
            "rule `key`: action view.cut is missing required payload field `itemRef`",
        ),
    ] {
        let root = package(dir.path(), rule);
        let (code, report) = run(&root);
        let errs = errors(&report);
        assert_eq!(code, 2, "{rule} must fail preflight; errors {errs:?}");
        assert!(
            errs.iter()
                .any(|e| e.contains("automationRule") && e.contains(needle)),
            "expected an error containing {needle:?}; got {errs:?}"
        );
    }
}

#[test]
fn a_rule_cycle_fails_preflight_with_the_cycle_named() {
    // WU5: SPEC §13.4 / AC-25 #3 — preflight REFUSES rules whose actions can
    // re-trigger themselves, directly or through other rules, over §13.4.1's
    // effects as data, and names the cycle.
    let dir = tempfile::tempdir().unwrap();
    for (rules, needle) in [
        (
            r#"{ "id": "loop", "trigger": { "kind": "stateChange", "params": { "field": "previewItem" } },
                 "action": { "command": "preview.set", "payload": { "itemRef": "A1" } } }"#,
            "automation rules form a cycle: `loop` → `loop` (`loop`: `preview.set` changes `previewItem`)",
        ),
        (
            r#"{ "id": "a", "trigger": { "kind": "stateChange", "params": { "field": "recordState" } },
                 "action": { "command": "view.take" } },
               { "id": "b", "trigger": { "kind": "stateChange", "params": { "field": "viewItem" } },
                 "action": { "command": "record.start" } }"#,
            "`a` → `b` → `a` (`a`: `view.take` changes `viewItem`; `b`: `record.start` changes `recordState`)",
        ),
        (
            // A deprecated alias reaches the same command, so it cannot hide
            // the edge; and the edge is deferred — the loop is one duration
            // long, and still a loop (WU5's decision).
            r#"{ "id": "replay", "trigger": { "kind": "mediaEnd", "params": { "itemRef": "A1" } },
                 "action": { "command": "program.cut", "payload": { "itemRef": "A1" } } }"#,
            "`replay` → `replay` (`replay`: `view.cut` schedules an item's end (mediaEnd, deferred))",
        ),
    ] {
        let root = package(dir.path(), rules);
        let (code, report) = run(&root);
        let errs = errors(&report);
        assert_eq!(code, 2, "{rules} must fail preflight; errors {errs:?}");
        assert!(
            errs.iter().any(|e| e.starts_with("automationRule: ")
                && e.contains(needle)
                && e.ends_with("(SPEC §13.4)")),
            "expected the cycle named, {needle:?}; got {errs:?}"
        );
    }

    // No cycle: the action raises a trigger no rule here listens for.
    let root = package(
        dir.path(),
        r#"{ "id": "rec", "trigger": { "kind": "stateChange", "params": { "field": "showState", "to": "RUNNING" } },
             "action": { "command": "record.start" } }"#,
    );
    let (code, report) = run(&root);
    assert_eq!(code, 0, "an acyclic rule passes; report {report}");
}

#[test]
fn the_two_key_passes_missing_edges_are_refused_and_every_field_still_admits_a_clean_rule() {
    // PR #34's fix round. The pass of 2026-09-29 found preflight admitting two
    // self-cycles (exit 0): `scene.arm` writes `previewItem` when the preview is
    // empty, and `show.stop` writes `streamState` and `recordState`. Both edges
    // are now in §13.4.1's data, and these pin the refusals.
    let dir = tempfile::tempdir().unwrap();
    for (rule, needle) in [
        (
            r#"{ "id": "arm", "trigger": { "kind": "stateChange", "params": { "field": "previewItem" } },
                 "action": { "command": "scene.arm", "payload": { "sceneId": "SCN" } } }"#,
            "`arm` → `arm` (`arm`: `scene.arm` changes `previewItem`)",
        ),
        (
            r#"{ "id": "stop", "trigger": { "kind": "stateChange", "params": { "field": "streamState" } },
                 "action": { "command": "show.stop" } }"#,
            "`stop` → `stop` (`stop`: `show.stop` changes `streamState`)",
        ),
        (
            r#"{ "id": "stop", "trigger": { "kind": "stateChange", "params": { "field": "recordState" } },
                 "action": { "command": "show.stop" } }"#,
            "`stop` → `stop` (`stop`: `show.stop` changes `recordState`)",
        ),
        (
            // The control: refused before the pass, and still.
            r#"{ "id": "pv", "trigger": { "kind": "stateChange", "params": { "field": "previewItem" } },
                 "action": { "command": "preview.set", "payload": { "itemRef": "A1" } } }"#,
            "`pv` → `pv` (`pv`: `preview.set` changes `previewItem`)",
        ),
    ] {
        let root = package(dir.path(), rule);
        let (code, report) = run(&root);
        let errs = errors(&report);
        assert_eq!(code, 2, "{rule} must fail preflight; errors {errs:?}");
        assert!(
            errs.iter()
                .any(|e| e.starts_with("automationRule: ") && e.contains(needle)),
            "expected the cycle named, {needle:?}; got {errs:?}"
        );
    }

    // A legitimate, non-cyclic rule on every field a stateChange rule can
    // name: the new edges must not turn clean rules into refusals.
    let clean = [
        "showState",
        "viewItem",
        "previewItem",
        "streamState",
        "recordState",
        "automationHold",
        "fallbackActive",
    ]
    .iter()
    .map(|f| {
        format!(
            r#"{{ "id": "on-{f}", "trigger": {{ "kind": "stateChange", "params": {{ "field": "{f}" }} }},
                 "action": {{ "command": "marker.add", "payload": {{ "name": "{f}" }} }} }}"#
        )
    })
    .chain(std::iter::once(
        r#"{ "id": "on-itemState", "trigger": { "kind": "stateChange", "params": { "field": "itemState", "itemRef": "A1" } },
             "action": { "command": "marker.add", "payload": { "name": "itemState" } } }"#
            .to_string(),
    ))
    .collect::<Vec<_>>()
    .join(",");
    let root = package(dir.path(), &clean);
    let (code, report) = run(&root);
    assert_eq!(
        code, 0,
        "a clean rule on each of the eight fields passes; report {report}"
    );
}
