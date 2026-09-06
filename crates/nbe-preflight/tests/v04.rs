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
    // View + preview + slate at 1920x1080 RGBA8 is EXACTLY 23 MiB before any
    // asset. `>= 20` was one-sided: every wrong-but-larger number satisfied it,
    // so the assertion could not tell a correct floor from an over-charge. The
    // floor is a closed-form number — assert the number.
    let vram = r["vramDemandMib"]
        .as_u64()
        .expect("vramDemandMib is a number");
    assert_eq!(
        vram, BASE_MIB,
        "the floor is view + preview + slate, RGBA8 at 1080p, and nothing else"
    );
    // The 8x8 slate is 256 B: real, and invisible at MiB resolution. That is
    // the truncation `audio_demand_is_pinned_at_a_mib_boundary` works around.
    assert_eq!(
        r["audioDemandMib"]
            .as_u64()
            .expect("audioDemandMib is a number"),
        0,
        "no audio-bearing asset is declared"
    );
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

    // 1. A 60-frame 1080p RGBA8 VRAM loop is 60 x 7.91 MiB = 475 MiB (§12.3's
    //    table), NOT the NV12 cost the old code guessed from `kind` (178 MiB).
    //    60 frames is chosen because it FITS: §12.5 caps an RGBA8 loop at
    //    floor(512 / 7.91) = 64 frames against §12.4's total budget, and a loop
    //    that does not fit is streamed, not charged — which is a different
    //    behaviour, tested below.
    let root = package_with_assets(
        dir.path(),
        r#", { "id": "L", "kind": "video", "source": "media/slate.png", "format": "h264",
              "loop": { "periodFrames": 60, "textureFormat": "rgba8", "cachePolicy": "vram", "vramBudgetMib": 512 } }"#,
    );
    let (_, report) = run(&root, &[]);
    let vram = report["resources"]["vramDemandMib"].as_u64().unwrap();
    assert!(
        (480..=520).contains(&vram),
        "60 RGBA8 1080p frames is ~475 MiB plus a ~23 MiB floor; got {vram}"
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

    // 3. BC7 is a quarter of RGBA8 (§12.3): 200 x 1.98 = 396 MiB. The band
    //    excludes both wrong answers — RGBA8's 1582 MiB and NV12's 593 MiB.
    let root = package_with_assets(
        dir.path(),
        r#", { "id": "L", "kind": "video", "source": "media/slate.png", "format": "h264",
              "loop": { "periodFrames": 200, "textureFormat": "bc7", "cachePolicy": "vram", "vramBudgetMib": 512 } }"#,
    );
    let (_, report) = run(&root, &[]);
    let vram = report["resources"]["vramDemandMib"].as_u64().unwrap();
    assert!(
        (400..=440).contains(&vram),
        "200 BC7 1080p frames is ~396 MiB plus the floor; got {vram}"
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

#[test]
fn an_auto_loop_the_budget_cannot_hold_is_streamed_not_charged() {
    // SPEC §12.5. The estimator skipped only the literal `cachePolicy:
    // "stream"`, so an `auto` loop the budget cannot hold was charged as
    // resident: 300 RGBA8 1080p frames reported 2396 MiB against a per-loop
    // budget of 256 MiB, which mandates streaming at 32 frames. `vramBudgetMib`
    // was never read at all. The number an operator plans against was 100x the
    // memory the engine would actually hold.
    let dir = tempfile::tempdir().unwrap();
    let root = package_with_assets(
        dir.path(),
        r#", { "id": "L", "kind": "video", "source": "media/slate.png", "format": "h264",
              "loop": { "periodFrames": 300, "textureFormat": "rgba8", "cachePolicy": "auto" } }"#,
    );
    let (code, report) = run(&root, &[]);
    let vram = report["resources"]["vramDemandMib"].as_u64().unwrap();
    assert_eq!(
        vram, BASE_MIB,
        "300 RGBA8 frames exceed §12.4's 256 MiB per-loop default, so §12.5 \
         streams the loop and it is not charged as resident"
    );
    // Streaming an oversized `auto` loop is lawful, not a failure: §12.5 fails
    // only a MANDATORY `vram` loop that cannot fit (tested next). The fixture's
    // "h264" source is a PNG, so `code` carries an unrelated decode error —
    // assert on the rule, not on the exit status.
    let _ = code;
    assert!(
        !report["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("loopBudget")),
        "an `auto` loop that streams is not a budget failure; got {:?}",
        report["errors"]
    );
}

#[test]
fn a_mandatory_vram_loop_that_does_not_fit_fails_preflight() {
    // SPEC §12.5: "Otherwise it MUST be streamed, unless `cachePolicy: vram` is
    // mandatory, in which case preflight MUST fail." A package that demands
    // residency the budget cannot give does not quietly stream — it does not
    // fit, and the operator must hear that before air rather than discover it
    // as a different picture than the one they authored.
    let dir = tempfile::tempdir().unwrap();
    let root = package_with_assets(
        dir.path(),
        r#", { "id": "L", "kind": "video", "source": "media/slate.png", "format": "h264",
              "loop": { "periodFrames": 300, "textureFormat": "rgba8", "cachePolicy": "vram" } }"#,
    );
    let (code, report) = run(&root, &[]);
    assert_eq!(code, 2, "a package that does not fit is not air-ready");
    let errors = report["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("loopBudget")
                && e.as_str().unwrap_or("").contains('L')),
        "the failure must name the asset and the rule; got {errors:?}"
    );
}

#[test]
fn a_period_past_the_frame_cap_is_named_not_a_panic() {
    // The schema declares `periodFrames` with `minimum: 1` and NO maximum, so a
    // package may legally declare a period near `u64::MAX`. The estimator's
    // `vram += period * per_frame` then overflowed: a panic in debug (exit 101,
    // no report written — which breaks P1's locked report-on-every-run), a
    // silent wrap in release. Saturating arithmetic alone would report a number
    // nobody can act on, so the package is refused by name against §12.4's cap.
    let dir = tempfile::tempdir().unwrap();
    let root = package_with_assets(
        dir.path(),
        r#", { "id": "L", "kind": "video", "source": "media/slate.png", "format": "h264",
              "loop": { "periodFrames": 18446744073709551615, "textureFormat": "rgba8" } }"#,
    );
    // `run` reads preflight_report.json: a panic fails here, before any
    // assertion, because a panicking preflight writes no report.
    let (code, report) = run(&root, &[]);
    assert_eq!(
        code, 2,
        "past the cap the package is refused (never exit 101)"
    );
    let errors = report["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("loopPeriod")
                && e.as_str().unwrap_or("").contains("900")),
        "the refusal must name the declared period and the cap; got {errors:?}"
    );
}

#[test]
fn audio_demand_is_pinned_at_a_mib_boundary() {
    // §8.4 residency: 48 kHz x 2 ch x f32 = 384 kB/s, so at 30 fps a frame of
    // clip audio is 12,800 B. The report carries MiB, and MiB truncation hides
    // every sub-MiB difference — 0 B and 128,000 B both display 0, so an
    // assertion on a small fixture cannot tell a working estimator from the one
    // that returned 0 for an hour of audio. Bracketing the 1 MiB boundary
    // restores byte resolution: 82 frames is 1,049,600 B (1 MiB) and 81 frames
    // is 1,036,800 B (0 MiB), so the estimator is pinned to within 12,800 B.
    let dir = tempfile::tempdir().unwrap();

    let below = package_with_assets(
        dir.path(),
        r#", { "id": "A", "kind": "audio", "source": "media/slate.png", "format": "wav",
              "expectedDurationFrames": 81 }"#,
    );
    let (_, report) = run(&below, &[]);
    assert_eq!(
        report["resources"]["audioDemandMib"].as_u64().unwrap(),
        0,
        "81 frames is 1,036,800 B — one frame short of a MiB"
    );

    let above = package_with_assets(
        dir.path(),
        r#", { "id": "A", "kind": "audio", "source": "media/slate.png", "format": "wav",
              "expectedDurationFrames": 82 }"#,
    );
    let (_, report) = run(&above, &[]);
    assert_eq!(
        report["resources"]["audioDemandMib"].as_u64().unwrap(),
        1,
        "82 frames is 1,049,600 B — one frame past a MiB"
    );

    // And linear above the boundary, which excludes both structural errors:
    // dropping the /frameRate division reports 3002 MiB, assuming mono 50.
    let long = package_with_assets(
        dir.path(),
        r#", { "id": "A", "kind": "audio", "source": "media/slate.png", "format": "wav",
              "expectedDurationFrames": 8200 }"#,
    );
    let (_, report) = run(&long, &[]);
    assert_eq!(
        report["resources"]["audioDemandMib"].as_u64().unwrap(),
        100,
        "8,200 frames is 104,960,000 B = 100 MiB"
    );
}

#[test]
fn an_uncountable_audio_duration_is_named_not_a_panic() {
    // `expectedDurationFrames` is the same unbounded input as `periodFrames`:
    // `minimum: 1`, no maximum. §8.4's residency is `frames * 48000 * 2 * 4`,
    // which panicked outright for a large declaration — and a panicking
    // preflight writes no report, which breaks P1's locked behaviour.
    let dir = tempfile::tempdir().unwrap();
    let root = package_with_assets(
        dir.path(),
        r#", { "id": "A", "kind": "audio", "source": "media/slate.png", "format": "wav",
              "expectedDurationFrames": 18446744073709551615 }"#,
    );
    // A panic fails inside `run`, before any assertion: no report is written.
    let (code, report) = run(&root, &[]);
    assert_eq!(code, 2, "a duration that cannot be counted is refused");
    let errors = report["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("audioDuration")),
        "the refusal must name the field and the asset; got {errors:?}"
    );
    // And the report still carries the resource block it always carries.
    assert!(report["resources"]["audioDemandMib"].is_number());
}
