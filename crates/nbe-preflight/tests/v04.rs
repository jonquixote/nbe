//! SPEC v0.4 preflight behaviours (Standards §2a: each fails when removed).
//!
//! §17.5 contradictory items, §12.11 package resource model, §7.15 the
//! house-rate warning that only fires when preflight was told a target.

use std::path::Path;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_nbe-preflight")
}

/// A minimal schema-valid package, with `items` spliced in verbatim.
fn package(dir: &Path, items: &str) -> std::path::PathBuf {
    let root = dir.join("pkg");
    std::fs::create_dir_all(root.join("media")).unwrap();
    let png = {
        let mut v = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([9, 9, 9, 255]),
        ))
        .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png)
        .unwrap();
        v
    };
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
      "rundown": {{ "id": "R", "items": [{items}] }}
    }}"#
    );
    std::fs::write(root.join("manifest.json"), manifest).unwrap();
    root
}

fn run(root: &Path, extra: &[&str]) -> (i32, serde_json::Value) {
    let mut cmd = Command::new(bin());
    cmd.arg("--package-path").arg(root);
    for a in extra {
        cmd.arg(a);
    }
    let out = cmd.output().expect("preflight runs");
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("preflight_report.json")).expect("report written"),
    )
    .expect("report is JSON");
    if std::env::var("V04_DEBUG").is_ok() {
        eprintln!("errors: {}", report["errors"]);
    }
    (out.status.code().unwrap_or(-1), report)
}

#[test]
fn a_slate_carrying_a_sceneref_is_a_preflight_failure() {
    // SPEC §17.5. This validated schema-clean and reported airReady: true in
    // v0.3 while being self-contradictory — the renderer short-circuited on
    // `kind` and drew the slate, and the audio path read `sceneRef` and
    // resolved the scene. A slate went to air with the previous item's audio.
    let dir = tempfile::tempdir().unwrap();
    let root = package(
        dir.path(),
        r#"{ "id": "A1", "kind": "slate", "sceneRef": "SCN" }"#,
    );
    let (code, report) = run(&root, &[]);
    assert_eq!(
        code, 2,
        "a contradictory item must fail preflight, not warn"
    );
    assert_eq!(report["airReady"], false);
    let errors = report["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("contradictoryItem")
                && e.as_str().unwrap_or("").contains("sceneRef")),
        "the error must name the item and the contradictory field; got {errors:?}"
    );
}

#[test]
fn every_kind_is_checked_not_only_the_two_the_spec_lists() {
    // §17.5 #1 is a GENERAL rule. The first implementation covered only the
    // two illustrative cases, leaving these legal — and `clipRef + sceneRef`
    // is the live one: the engine fills `item_scene` from `sceneRef`
    // regardless of kind, so it draws the scene and resolves its audio for an
    // item the manifest calls a clip.
    let dir = tempfile::tempdir().unwrap();
    for (kind, field, value) in [
        ("clipRef", "sceneRef", "\"SCN\""),
        ("liveRef", "sceneRef", "\"SCN\""),
        ("liveRef", "assetId", "\"slate\""),
        ("clipRef", "sourceId", "\"cam1\""),
    ] {
        let root = package(
            dir.path(),
            &format!(r#"{{ "id": "A1", "kind": "{kind}", "{field}": {value} }}"#),
        );
        let (code, report) = run(&root, &[]);
        assert_eq!(
            code, 2,
            "a {kind} item carrying {field} must fail preflight; report {report}"
        );
        assert!(
            report["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e.as_str().unwrap_or("").contains("contradictoryItem")),
            "the error must name the contradiction for {kind} + {field}"
        );
    }
}

#[test]
fn each_kind_may_carry_its_own_field() {
    // The other half: the general rule must not forbid the legitimate shapes,
    // or it is a rule nobody can satisfy.
    let dir = tempfile::tempdir().unwrap();
    for item in [
        r#"{ "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }"#,
        r#"{ "id": "A1", "kind": "clipRef", "assetId": "slate" }"#,
        r#"{ "id": "A1", "kind": "liveRef", "sourceId": "cam1" }"#,
        r#"{ "id": "A1", "kind": "slate" }"#,
    ] {
        let root = package(dir.path(), item);
        let (code, report) = run(&root, &[]);
        assert_eq!(code, 0, "{item} must remain air-ready; report {report}");
    }
}

#[test]
fn a_well_formed_item_is_not_flagged_as_contradictory() {
    // The other half: the check must not fire on a legitimate package, or it
    // would be a rule nobody could satisfy.
    let dir = tempfile::tempdir().unwrap();
    let root = package(
        dir.path(),
        r#"{ "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }"#,
    );
    let (code, report) = run(&root, &[]);
    assert_eq!(code, 0, "a well-formed package must remain air-ready");
    assert_eq!(report["errors"].as_array().unwrap().len(), 0);
}

#[test]
fn the_report_always_carries_the_resource_block() {
    // SPEC §12.11.3 #1 / §19.2.1: reported unconditionally. A number an
    // operator can read is the deliverable, not only a threshold that trips.
    let dir = tempfile::tempdir().unwrap();
    let root = package(
        dir.path(),
        r#"{ "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }"#,
    );
    let (_, report) = run(&root, &[]);
    let r = &report["resources"];
    assert!(r.is_object(), "resources must be present; got {report}");
    assert_eq!(r["declaredHouseRate"], 30);
    // View + preview + slate at 1920x1080 RGBA8 is ~23 MiB before any asset.
    let vram = r["vramDemandMib"]
        .as_u64()
        .expect("vramDemandMib is a number");
    assert!(
        vram >= 20,
        "render targets and the slate alone exceed 20 MiB at 1080p; got {vram}"
    );
    assert!(r["audioDemandMib"].is_number());
}

/// A package whose assets are spliced in verbatim, for resource arithmetic.
fn package_with_assets(dir: &Path, assets: &str) -> std::path::PathBuf {
    let root = dir.join("respkg");
    std::fs::create_dir_all(root.join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        200,
        100,
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
      "assets": [
        {{ "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" }}
        {assets}
      ],
      "scenes": [{{ "id": "SCN", "elements": [
        {{ "id": "bg", "kind": "graphic", "z": 0, "templateId": "TPL" }}
      ] }}],
      "templates": [{{ "id": "TPL", "kind": "generic" }}],
      "rundown": {{ "id": "R", "items": [
        {{ "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }}
      ] }}
    }}"#
    );
    std::fs::write(root.join("manifest.json"), manifest).unwrap();
    root
}

/// The unconditional floor: view + preview + slate, RGBA8 at 1920x1080.
const BASE_MIB: u64 = (1920 * 1080 * 4 * 3) / (1024 * 1024); // 23

#[test]
fn the_resource_numbers_are_right_not_merely_present() {
    // SPEC §12.11.1. The first implementation produced a number for every
    // package and the number was wrong: 2.6x under on an RGBA8 loop, 38x over
    // on a streamed loop, 13x over on images, and 0 for an hour of audio.
    // Asserting presence passed all four. These assert the arithmetic.
    let dir = tempfile::tempdir().unwrap();

    // 1. A 300-frame 1080p RGBA8 VRAM loop is 300 x 7.91 MiB = 2373 MiB
    //    (§12.3's table), NOT the NV12 cost the old code guessed from `kind`.
    let root = package_with_assets(
        dir.path(),
        r#", { "id": "L", "kind": "video", "source": "media/slate.png", "format": "h264",
              "loop": { "periodFrames": 300, "textureFormat": "rgba8", "cachePolicy": "vram" } }"#,
    );
    let (_, report) = run(&root, &[]);
    let vram = report["resources"]["vramDemandMib"].as_u64().unwrap();
    assert!(
        (2300..=2450).contains(&vram),
        "300 RGBA8 1080p frames is ~2373 MiB plus a ~23 MiB floor; got {vram}"
    );

    // 2. The SAME loop declared `stream` is not VRAM-resident at all (§12.2),
    //    so it must not be charged. The old code charged it anyway.
    let root = package_with_assets(
        dir.path(),
        r#", { "id": "L", "kind": "video", "source": "media/slate.png", "format": "h264",
              "loop": { "periodFrames": 300, "textureFormat": "rgba8", "cachePolicy": "stream" } }"#,
    );
    let (_, report) = run(&root, &[]);
    let vram = report["resources"]["vramDemandMib"].as_u64().unwrap();
    assert!(
        vram <= BASE_MIB + 5,
        "a streamed loop holds a read-ahead window, not its period; got {vram} MiB"
    );

    // 3. BC7 is a quarter of RGBA8 (§12.3): 300 x 1.98 = 594 MiB.
    let root = package_with_assets(
        dir.path(),
        r#", { "id": "L", "kind": "video", "source": "media/slate.png", "format": "h264",
              "loop": { "periodFrames": 300, "textureFormat": "bc7", "cachePolicy": "vram" } }"#,
    );
    let (_, report) = run(&root, &[]);
    let vram = report["resources"]["vramDemandMib"].as_u64().unwrap();
    assert!(
        (560..=680).contains(&vram),
        "300 BC7 1080p frames is ~594 MiB plus the floor; got {vram}"
    );
}

#[test]
fn an_images_cost_is_its_own_size_not_the_house_frame() {
    // The old code charged every image a full 1920x1080 RGBA8 frame because it
    // never probed image dimensions — 40 small logos read as 348 MiB instead
    // of 27. The fixture's slate is 200x100 = 0.08 MiB.
    let dir = tempfile::tempdir().unwrap();
    let root = package_with_assets(dir.path(), "");
    let (_, report) = run(&root, &[]);
    let vram = report["resources"]["vramDemandMib"].as_u64().unwrap();
    assert!(
        vram <= BASE_MIB + 1,
        "a 200x100 image must not be charged a 1080p frame; got {vram} MiB \
         against a {BASE_MIB} MiB floor"
    );
}

#[test]
fn the_resource_block_is_present_even_when_nothing_is_declared() {
    // §19.2.1: REQUIRED and always populated. `declaredHouseRate` was
    // skip_serializing_if=None, so a report could omit it — and an absent
    // field is a different failure from a null one.
    let dir = tempfile::tempdir().unwrap();
    let root = package_with_assets(dir.path(), "");
    let (_, report) = run(&root, &[]);
    let r = &report["resources"];
    assert!(
        r.get("declaredHouseRate").is_some(),
        "field must be present"
    );
    assert!(r.get("vramDemandMib").is_some());
    assert!(r.get("audioDemandMib").is_some());
}

#[test]
fn preflight_warns_on_a_house_rate_mismatch_and_only_then() {
    // SPEC §7.15 #2. Preflight validates a package in isolation, so it warns
    // when TOLD a target rate that differs — and must not warn merely because
    // a package declares a rate, which would make every package non-air-ready
    // and teach operators to pass --allow-warnings by reflex.
    let dir = tempfile::tempdir().unwrap();
    let root = package(
        dir.path(),
        r#"{ "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }"#,
    );

    let (code, report) = run(&root, &["--house-rate", "30"]);
    assert_eq!(code, 0, "a matching rate must stay air-ready");
    assert!(
        !report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("houseRate")),
        "a matching rate must not warn"
    );

    let (code, report) = run(&root, &["--house-rate", "60"]);
    assert_eq!(code, 1, "a mismatch warns; it never fails (SPEC §7.15 #2)");
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("houseRate")),
        "a mismatch must warn, naming both rates; got {:?}",
        report["warnings"]
    );
}
