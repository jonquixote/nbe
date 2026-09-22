//! Typed-model coverage for nbe-core.
//!
//! The round-trip test is the point: every schema/model mismatch that
//! Prompt 01b Step 1 fixed (numeric frameRate, ClockFormat, TextureFormat)
//! is pinned by it. If the typed model drifts from
//! `schemas/manifest.v0.4.json` again, this file fails first.

use std::path::PathBuf;

use nbe_core::{
    validate_manifest, LoudnessSummary, Manifest, PluginReport, PreflightReport, SceneReport,
};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/valid_show_v0.3")
}

fn fixture_json() -> serde_json::Value {
    let raw = std::fs::read_to_string(fixture_dir().join("manifest.json"))
        .expect("v0.3 fixture manifest.json must exist");
    serde_json::from_str(&raw).expect("v0.3 fixture must be valid JSON")
}

/// THE test of this prompt: deserialize the normative fixture into the
/// typed model, serialize it back, and re-validate against the schema.
/// Any serde/schema drift (rename, numeric enum, optionality) fails here.
#[test]
fn fixture_round_trips_through_typed_model_and_revalidates() {
    let json = fixture_json();
    let manifest: Manifest = serde_json::from_value(json)
        .expect("schema-valid fixture MUST deserialize into the typed Manifest model");
    let republished = serde_json::to_value(&manifest).expect("typed manifest must serialize");
    validate_manifest(&republished)
        .expect("a manifest that round-tripped through the typed model must re-validate");
}

/// Explicit defaults: a manifest that writes default values explicitly
/// (numeric frameRate, "HH:mm:ss" clock format, "auto" texture format)
/// must deserialize cleanly.
#[test]
fn explicit_default_spellings_deserialize() {
    let mut json = fixture_json();
    // frameRate is already numeric 30 in the fixture. Add the others.
    json["scenes"] = serde_json::json!([
        {
            "id": "SCN_A1",
            "elements": [
                {
                    "id": "clk",
                    "kind": "clock",
                    "z": 5,
                    "clock": { "format": "HH:mm:ss" }
                },
                {
                    "id": "bg",
                    "kind": "videoLoop",
                    "z": 0,
                    "assetId": "A1_clip",
                    "loop": { "periodFrames": 30, "textureFormat": "auto", "cachePolicy": "auto" }
                }
            ]
        }
    ]);
    let manifest: Manifest = serde_json::from_value(json)
        .expect("manifest with explicit default spellings must deserialize");
    // frameRate read back as the typed enum.
    assert_eq!(
        u32::from(manifest.show.video.frame_rate),
        30,
        "numeric frameRate must deserialize to FrameRate::Fps30"
    );
}

/// Lock the PreflightReport JSON shape against SPEC Section 19.2:
/// exact camelCase field names, top level and per nested entry.
#[test]
fn preflight_report_shape_matches_spec_19_2() {
    let report = PreflightReport {
        manifest_valid: true,
        air_ready: true,
        errors: vec![],
        warnings: vec![],
        assets: vec![nbe_core::AssetReport {
            id: "A1".into(),
            kind: Some("video".into()),
            exists: true,
            sha256_ok: Some(true),
            decode_first_frame_ok: Some(true),
            decode_last_frame_ok: Some(true),
            cfr: Some(true),
            frame_rate: Some(30.0),
            width: Some(1920),
            height: Some(1080),
            duration_frames: Some(900),
            cadence_ok: Some(true),
            loudness: Some(LoudnessSummary {
                integrated_lufs: Some(-16.1),
                true_peak_dbtp: Some(-1.7),
            }),
        }],
        loops: vec![nbe_core::LoopReport {
            asset_id: "globe_loop".into(),
            period_frames: Some(300),
            seamless: Some(true),
            selected_texture_format: Some("bc7".into()),
            frame_cost_mib: Some(1.98),
            effective_budget_mib: Some(256),
            max_frames_by_budget: Some(129),
            cache_policy_selected: Some("stream".into()),
            vram_resident: Some(false),
            reason: Some("periodFrames exceeds maxFramesByBudget".into()),
        }],
        scenes: vec![SceneReport {
            scene_id: "SCN_A1".into(),
            references_ok: true,
            dag_ok: true,
        }],
        plugins: vec![PluginReport {
            plugin_id: "lowerthird_anim".into(),
            sandbox_ok: true,
        }],
        contact_sheet: Some("contact_sheet.jpg".into()),
        // SPEC §19.2.1 (v0.4): always populated, never optional.
        resources: nbe_core::ResourceReport {
            vram_demand_mib: 412,
            audio_demand_mib: 22,
            declared_house_rate: Some(30),
        },
    };
    let v = serde_json::to_value(&report).expect("report must serialize");

    // Top-level names (SPEC 19.2), in order-insensitive assertion.
    for key in [
        "manifestValid",
        "airReady",
        "errors",
        "warnings",
        "assets",
        "loops",
        "scenes",
        "plugins",
        "contactSheet",
    ] {
        assert!(v.get(key).is_some(), "report missing top-level key {key}");
    }
    assert_eq!(v["contactSheet"], "contact_sheet.jpg");
    // SPEC §19.2.1: the resource block and its exact camelCase spelling.
    assert_eq!(v["resources"]["vramDemandMib"], 412);
    assert_eq!(v["resources"]["audioDemandMib"], 22);
    assert_eq!(v["resources"]["declaredHouseRate"], 30);

    let asset = &v["assets"][0];
    for key in [
        "id",
        "kind",
        "exists",
        "sha256Ok",
        "decodeFirstFrameOk",
        "decodeLastFrameOk",
        "cfr",
        "frameRate",
        "width",
        "height",
        "durationFrames",
        "cadenceOk",
        "loudness",
    ] {
        assert!(asset.get(key).is_some(), "assets[] missing key {key}");
    }
    assert!(asset["loudness"].get("integratedLufs").is_some());
    assert!(asset["loudness"].get("truePeakDbtp").is_some());

    let loop_ = &v["loops"][0];
    assert!(loop_.get("assetId").is_some());
    assert!(loop_.get("periodFrames").is_some());
    assert_eq!(loop_["periodFrames"], 300);

    let scene = &v["scenes"][0];
    assert!(scene.get("sceneId").is_some());
    assert!(scene.get("referencesOk").is_some());
    assert!(scene.get("dagOk").is_some());

    let plugin = &v["plugins"][0];
    assert!(plugin.get("pluginId").is_some());
    assert!(plugin.get("sandboxOk").is_some());
}

/// The schema's transport enum and the typed model's are one list, audited.
///
/// **Added because a falsification found the narrowing unguarded.** SPEC v0.4.5
/// narrowed `outputs.stream.protocol` to `["rtmp"]`, and widening it back to
/// `["rtmp","srt","whip"]` broke nothing: `validate::check_transport` refuses
/// `srt` and `whip` by name whatever the schema says, so the schema half of the
/// refusal had no test that bound it (§2a rule 4 — a behaviour whose removal
/// breaks nothing is untested).
///
/// Same shape as `quality_profile_matches_the_manifest_schema_enum` in
/// `nbe-protocol`'s mirror: the schema is normative, the model follows, and
/// this fails if either side moves alone.
#[test]
fn stream_protocol_matches_the_manifest_schema_enum() {
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/manifest.v0.4.json"),
        )
        .expect("the embedded schema is readable"),
    )
    .expect("the schema parses");
    let declared: Vec<String> = schema["$defs"]["OutputDefaults"]["properties"]["stream"]
        ["properties"]["protocol"]["enum"]
        .as_array()
        .expect("the protocol enum is an array")
        .iter()
        .map(|v| v.as_str().expect("enum values are strings").to_string())
        .collect();

    // The model's side, spelled the way serde spells it on the wire.
    let ours: Vec<String> = [nbe_core::StreamProtocol::Rtmp]
        .iter()
        .map(|p| {
            serde_json::to_value(p)
                .expect("serializes")
                .as_str()
                .expect("a string")
                .to_string()
        })
        .collect();

    assert_eq!(
        declared, ours,
        "the schema's transport enum and the typed model's must be one list; \
         widen one without the other and a manifest deserializes into a variant \
         nothing can serve, or is refused for a transport the model supports"
    );
}
