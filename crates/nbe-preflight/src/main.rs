//! nbe-preflight: CI-runnable show-package validator.
//! SPEC Section 19: exit 0 air-ready, 1 warnings only, 2 errors.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use nbe_core::{AssetReport, PreflightReport, ValidationError};
use std::collections::HashSet;

/// How many frames a preflight decode pass will read per asset. Preflight is
/// allowed to be slow; it is not allowed to be unbounded.
const DECODE_FRAME_LIMIT: usize = 100_000;

/// SPEC §12.4's absolute short-loop frame cap, widened for the comparison
/// against a `periodFrames` the schema leaves unbounded.
///
/// The number itself lives in `nbe_core::loop_cache` with the rule it
/// parameterises, so preflight and the engine cannot hold different §12.4s.
const ABSOLUTE_LOOP_FRAME_CAP: u64 = nbe_core::loop_cache::ABSOLUTE_LOOP_FRAME_CAP as u64;

/// SPEC §8.4 residency for one second of clip audio: 48 kHz x 2 ch x f32.
const AUDIO_BYTES_PER_SECOND: u64 = 48_000 * 2 * 4;

/// The pixel dimensions one frame of an asset occupies (SPEC §12.11.1).
///
/// The chain is fixed **here**, and no caller may supply its own: the asset's
/// **probed** size where preflight measured it, otherwise the **house** frame
/// from `show.video`. (The schema declares no per-asset resolution, so there is
/// no third rung to consult — `show.video` is the declaration.) The house frame
/// over-states a small asset rather than under-stating a large one, which is
/// the direction a resource report should err.
///
/// This was two chains. The §12.5 gate fell back to a hardcoded 1920x1080 while
/// the estimator fell back to the house frame, so on a 4K package with an
/// unprobed loop asset the gate planned at 1080p — where 30 RGBA8 frames fit
/// the 256 MiB default — while the estimator planned at 4K, where 7 do: the
/// package came back `airReady: true` with §12.5's mandatory-`vram` failure
/// silenced and the loop charged nothing. Both halves called one shared
/// function and still disagreed, because a shared function is only shared if
/// its arguments are too.
fn asset_dimensions(
    id: &str,
    manifest: &serde_json::Value,
    report: &nbe_core::PreflightReport,
) -> (u64, u64) {
    let video = manifest.get("show").and_then(|s| s.get("video"));
    let house = |field: &str, fallback: u64| {
        video
            .and_then(|v| v.get(field))
            .and_then(|v| v.as_u64())
            .unwrap_or(fallback)
    };
    let probed = report.assets.iter().find(|a| a.id == id);
    (
        probed
            .and_then(|a| a.width)
            .map(u64::from)
            .unwrap_or_else(|| house("width", 1920)),
        probed
            .and_then(|a| a.height)
            .map(u64::from)
            .unwrap_or_else(|| house("height", 1080)),
    )
}

/// SPEC §12.5's residency decision for one declared loop.
///
/// Both callers — the estimator ("what does this package demand?") and the
/// §12.5 mandatory-`vram` check ("does this package fit?") — must answer from
/// the same plan, or preflight contradicts itself inside a single run. The plan
/// itself lives in `nbe-core` so preflight and the engine cannot disagree
/// either. Every input the plan needs is assembled here, from the manifest and
/// the report: a caller passes the asset, not the arithmetic.
fn loop_plan(
    asset: &serde_json::Value,
    lm: &serde_json::Value,
    manifest: &serde_json::Value,
    report: &nbe_core::PreflightReport,
) -> nbe_core::loop_cache::CachePlan {
    let id = asset.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let (width, height) = asset_dimensions(id, manifest, report);
    use nbe_core::loop_cache::{CacheBudget, CacheTextureFormat, LoopSpec};
    let kind = asset.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let declared_format = lm
        .get("textureFormat")
        .and_then(|v| v.as_str())
        .and_then(CacheTextureFormat::from_declared);
    let has_alpha = kind == "alphaVideo";
    nbe_core::loop_cache::plan(
        LoopSpec {
            width: width.min(u32::MAX as u64) as u32,
            height: height.min(u32::MAX as u64) as u32,
            period_frames: lm
                .get("periodFrames")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            has_alpha,
            // Absent a declaration, §12.3's ladder: an opaque source takes the
            // NV12 rung, alpha content takes RGBA8.
            yuv_sampling: !has_alpha,
            gop_frames: 0,
            declared_format,
        },
        // §12.4's table, with the manifest's ceiling where it declares one.
        // Built by the same constructor the engine uses.
        CacheBudget::from_manifest(
            lm.get("vramBudgetMib")
                .and_then(|v| v.as_u64())
                .map(|v| v.min(u32::MAX as u64) as u32),
        ),
    )
}

/// The decode-derived fields of an `AssetReport` (SPEC §19.2).
#[derive(Default)]
struct DecodeFacts {
    first_frame_ok: Option<bool>,
    last_frame_ok: Option<bool>,
    cfr: Option<bool>,
    frame_rate: Option<f64>,
    width: Option<u32>,
    height: Option<u32>,
    duration_frames: Option<u32>,
}

impl DecodeFacts {
    fn from_probe(p: &nbe_decode::AssetProbe) -> Self {
        Self {
            // A probe decodes every frame, so reaching the end means both the
            // first and the last frame decoded.
            first_frame_ok: Some(true),
            last_frame_ok: Some(true),
            cfr: Some(p.cfr),
            frame_rate: Some(p.measured_frame_rate),
            width: Some(p.width),
            height: Some(p.height),
            duration_frames: Some(p.frame_count as u32),
        }
    }
}

/// Validate an NBE show package for air-readiness.
#[derive(Parser, Debug)]
#[command(name = "nbe-preflight", version, about)]
struct Args {
    /// Path to the show package directory containing manifest.json.
    #[arg(long)]
    package_path: PathBuf,

    /// Exit 0 even when warnings are present (exit 2 still on errors).
    #[arg(long)]
    allow_warnings: bool,

    /// The house rate of the engine this package is destined for, if known.
    ///
    /// SPEC §7.15: preflight validates a package in isolation and cannot know
    /// the target, so it WARNS on a mismatch and never fails. `show.load` —
    /// which knows both the package and the running engine — refuses.
    #[arg(long)]
    house_rate: Option<u32>,
}

/// Registered §16 commands: the keys of `CommandPayloadSchemas` in
/// `packages/control-plane/src/protocol.ts`. A control binding's `action`
/// must name one of these; anything else cannot execute and fails preflight
/// (Prompt 08 work item 5). Keep in sync with protocol.ts when §16 grows.
const REGISTERED_COMMANDS: &[&str] = &[
    "show.load",
    "show.preflight",
    "show.start",
    "show.stop",
    "show.unload",
    "preview.set",
    "view.take",
    "view.cut",
    "view.fallback",
    "scene.arm",
    "scene.apply",
    "item.arm",
    "item.unarm",
    "item.stop",
    "item.reset",
    "element.toggle",
    "element.set",
    "graphic.show",
    "graphic.hide",
    "graphic.update",
    "breaking.show",
    "breaking.hide",
    "overlay.show",
    "overlay.hide",
    "ticker.setSource",
    "ticker.override",
    "ticker.clearOverride",
    "ticker.refreshRss",
    "soundboard.play",
    "soundboard.stop",
    "soundboard.stopAll",
    "audio.bus.set",
    "audio.duck",
    "guest.mute",
    "guest.connect",
    "guest.disconnect",
    "guest.setLayout",
    "guest.placeholder",
    "guest.configureReturn",
    "guest.getTurn",
    "automation.enable",
    "automation.disable",
    "automation.hold",
    "snapshot.save",
    "snapshot.recall",
    "marker.add",
    "plugin.reload",
    "clock.configure",
    "record.start",
    "record.stop",
    "stream.start",
    "stream.stop",
    "system.status",
    "system.telemetry.subscribe",
    "system.telemetry.unsubscribe",
];

/// Minimal required payload keys per action family. `view.take` takes none
/// (every field is optional); anything not listed here is accepted as long
/// as it is an object — full payload validation belongs to the §16 schema,
/// not preflight. Absent payload counts as `{}`: a command whose keys are
/// required fails with no payload just as with an empty one.
fn required_payload_keys(action: &str) -> &'static [&'static str] {
    match action {
        "show.load" => &["packagePath"],
        "preview.set" => &["itemRef"],
        "view.cut" => &["itemRef"],
        "scene.arm" | "scene.apply" => &["sceneId"],
        "item.arm" | "item.unarm" | "item.stop" | "item.reset" => &["itemId"],
        "element.toggle" | "element.set" => &["elementId"],
        "graphic.show" => &["templateId"],
        "graphic.update" => &["elementId"],
        "breaking.show" => &["headline"],
        "overlay.show" | "overlay.hide" => &["overlayId"],
        "ticker.setSource" => &["source"],
        "soundboard.play" => &["assetId"],
        "audio.bus.set" | "audio.duck" => &["bus"],
        "guest.mute"
        | "guest.connect"
        | "guest.disconnect"
        | "guest.setLayout"
        | "guest.placeholder"
        | "guest.configureReturn"
        | "guest.getTurn" => &["guestId"],
        "automation.enable" | "automation.disable" => &["ruleId"],
        "snapshot.save" | "snapshot.recall" | "marker.add" => &["name"],
        "plugin.reload" => &["pluginId"],
        "clock.configure" => &["elementId"],
        _ => &[],
    }
}

/// Assumption 17 runtime parity: `resolveCommand` accepts `program.*` and
/// `layer.*` deprecated aliases, so preflight must too — a binding the bus
/// would execute must not fail preflight. Returns the canonical action.
fn canonical_action(action: &str) -> Option<&'static str> {
    if let Some(hit) = REGISTERED_COMMANDS.iter().find(|c| **c == action) {
        return Some(hit);
    }
    for (from, to) in [("program.", "view."), ("layer.", "element.")] {
        if let Some(rest) = action.strip_prefix(from) {
            if let Some(hit) = REGISTERED_COMMANDS
                .iter()
                .find(|c| **c == format!("{to}{rest}"))
            {
                return Some(hit);
            }
        }
    }
    None
}

fn run(package_path: &Path, house_rate: Option<u32>) -> Result<(PreflightReport, bool)> {
    let mut report = PreflightReport::default();
    let manifest_path = package_path.join("manifest.json");

    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read manifest at {}", manifest_path.display()))?;
    let manifest_json: serde_json::Value = serde_json::from_str(&raw).with_context(|| {
        format!(
            "manifest.json is not valid JSON: {}",
            manifest_path.display()
        )
    })?;

    // Version gate first, then schema (SPEC 6.7, Assumption 18, AC-28).
    let mut had_errors = false;
    match nbe_core::validate_manifest(&manifest_json) {
        Ok(()) => {
            report.manifest_valid = true;
        }
        Err(e) => {
            report.manifest_valid = false;
            had_errors = true;
            match &e {
                ValidationError::MigrationRequired { .. } => {
                    report.push_error(format!("migrationRequired: {e}"));
                }
                ValidationError::MissingVersion => {
                    report.push_error(format!("malformedManifest: {e}"));
                }
                ValidationError::SchemaViolation { .. } => {
                    report.push_error(e.to_string());
                }
                ValidationError::SchemaCompile(_) => {
                    report.push_error(format!("internal: {e}"));
                }
            }
        }
    }

    // The house video format the assets are measured against (SPEC §6.2).
    let house_resolution = manifest_json
        .get("show")
        .and_then(|s| s.get("video"))
        .and_then(|v| {
            Some((
                v.get("width")?.as_u64()? as u32,
                v.get("height")?.as_u64()? as u32,
            ))
        });
    let house_frame_rate = manifest_json
        .get("show")
        .and_then(|s| s.get("video"))
        .and_then(|v| v.get("frameRate"))
        .and_then(|v| v.as_u64())
        .map(|r| r as u32);

    // Asset existence: only meaningful once schema validation passes,
    // but we check regardless so the report is complete.
    if let Some(assets) = manifest_json.get("assets").and_then(|a| a.as_array()) {
        for asset in assets {
            let id = asset
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("<unknown>")
                .to_string();
            let kind = asset.get("kind").and_then(|k| k.as_str()).map(String::from);
            let source = asset.get("source").and_then(|s| s.as_str());
            let exists = match source {
                Some(src) => package_path.join(src).exists(),
                None => false,
            };
            if !exists {
                had_errors = true;
                report.push_error(format!(
                    "missing asset: id \"{id}\" source {:?} not found relative to package root",
                    source
                ));
            }
            // Decode-based checks (Prompt 05 Step 7, SPEC §19): only for
            // media that exists and claims to be video.
            let mut decoded = DecodeFacts::default();
            // §12.11.1 needs an image's OWN dimensions: a 200x100 logo is
            // 0.08 MiB, not the 7.91 MiB of a house-resolution frame. Reading
            // only the header keeps this cheap — no decode, no pixels.
            if exists && kind.as_deref() == Some("image") {
                // Non-fatal by design. An unreadable image is arguably a
                // preflight failure, but v0.4's content list does not
                // authorise a new failure mode — that would be enlarging the
                // revision past its scope. If the header will not parse we
                // fall back to the house frame, which over-states demand
                // rather than under-stating it.
                if let Ok((w, h)) =
                    image::image_dimensions(package_path.join(source.unwrap_or_default()))
                {
                    decoded.width = Some(w);
                    decoded.height = Some(h);
                }
            }

            let is_video = matches!(kind.as_deref(), Some("video") | Some("alphaVideo"));
            if exists && is_video {
                let path = package_path.join(source.unwrap_or_default());
                match nbe_decode::probe_asset(&path, DECODE_FRAME_LIMIT) {
                    Ok(probe) => {
                        decoded = DecodeFacts::from_probe(&probe);
                        // AC-3: a variable frame rate is an error, not a
                        // warning — VFR media cannot hold cadence on air.
                        if !probe.cfr {
                            had_errors = true;
                            report.push_error(format!(
                                "vfr: asset \"{id}\" is not constant frame rate (measured {:.3} fps); SPEC §6.2 requires CFR",
                                probe.measured_frame_rate
                            ));
                        }
                        if let Some((want_w, want_h)) = house_resolution {
                            if probe.width != want_w || probe.height != want_h {
                                had_errors = true;
                                report.push_error(format!(
                                    "resolution: asset \"{id}\" is {}x{}, house format is {want_w}x{want_h}",
                                    probe.width, probe.height
                                ));
                            }
                        }
                        // Cadence: a source slower than the house rate is
                        // legal and held (SPEC §18); a source FASTER than the
                        // house rate loses frames, which is a warning an
                        // operator should see before air.
                        if let Some(house_rate) = house_frame_rate {
                            let src = probe.nominal_frame_rate.round() as u32;
                            if src > house_rate {
                                report.push_warning(format!(
                                    "cadence: asset \"{id}\" is {src} fps against a {house_rate} fps house rate; frames will be dropped"
                                ));
                            }
                        }
                        // Declared duration versus decoded reality.
                        if let Some(expected) =
                            asset.get("expectedDurationFrames").and_then(|v| v.as_u64())
                        {
                            if expected != probe.frame_count {
                                report.push_warning(format!(
                                    "duration: asset \"{id}\" declares {expected} frames, decoded {}",
                                    probe.frame_count
                                ));
                            }
                        }
                        // Declared loop period versus reality (SPEC §12.10).
                        if let Some(period) = asset
                            .get("loop")
                            .and_then(|l| l.get("periodFrames"))
                            .and_then(|v| v.as_u64())
                        {
                            if period != probe.frame_count {
                                had_errors = true;
                                report.push_error(format!(
                                    "loop: asset \"{id}\" declares periodFrames {period}, decoded {} frames",
                                    probe.frame_count
                                ));
                            }
                        }
                        // Alpha presence for assets that promise it.
                        if kind.as_deref() == Some("alphaVideo") && !probe.has_alpha {
                            had_errors = true;
                            report.push_error(format!(
                                "alpha: asset \"{id}\" is declared alphaVideo but carries no alpha"
                            ));
                        }
                    }
                    Err(e) => {
                        had_errors = true;
                        report.push_error(format!("decode: asset \"{id}\" failed to decode: {e}"));
                    }
                }
            }

            report.assets.push(AssetReport {
                id,
                kind,
                exists,
                decode_first_frame_ok: decoded.first_frame_ok,
                decode_last_frame_ok: decoded.last_frame_ok,
                cfr: decoded.cfr,
                frame_rate: decoded.frame_rate,
                width: decoded.width,
                height: decoded.height,
                duration_frames: decoded.duration_frames,
                ..Default::default()
            });
        }
    }

    // SPEC §17.5 (v0.4): an Item carrying fields belonging to another `kind`
    // is a contradiction, not a harmless extra. `{"kind":"slate","sceneRef":…}`
    // validated schema-clean and preflighted air-ready while the renderer drew
    // a slate and the audio path resolved the scene — a slate on air with the
    // previous item's audio. There is no correct precedence; the package is
    // invalid.
    for item in manifest_json
        .get("rundown")
        .and_then(|r| r.get("items"))
        .and_then(|i| i.as_array())
        .into_iter()
        .flatten()
    {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>");
        let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        // §17.5 #1 is a GENERAL rule: an Item MUST NOT carry fields belonging
        // to a kind other than its own. The first implementation covered only
        // the two cases the spec lists as illustrations, which left
        // `clipRef + sceneRef` legal — and that one is live: `index_sequence`
        // fills `item_scene` from `sceneRef` regardless of kind, and
        // `drawn_elements` special-cases only `slate`, so the engine draws the
        // scene and resolves its audio for an item the manifest calls a clip.
        //
        // Expressed as "which field belongs to which kind", so a new kind
        // cannot quietly inherit permission to carry everything.
        let owned: &[&str] = match kind {
            "sceneRef" => &["sceneRef"],
            "clipRef" => &["assetId"],
            "liveRef" => &["sourceId"],
            "slate" => &[],
            // An unknown kind is the schema's problem, not this check's.
            _ => continue,
        };
        let offending: Vec<&str> = ["sceneRef", "assetId", "sourceId"]
            .into_iter()
            .filter(|f| !owned.contains(f))
            .collect();
        for field in &offending {
            if item.get(*field).is_some() {
                had_errors = true;
                report.push_error(format!(
                    "contradictoryItem: item \"{id}\" is kind \"{kind}\" but carries \"{field}\" (SPEC §17.5)"
                ));
            }
        }
    }

    // SPEC §12.4: the absolute short-loop frame cap. Checked before the
    // resource arithmetic so a package past the bound is refused by name
    // rather than saturating into a number that means nothing.
    for asset in manifest_json
        .get("assets")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
    {
        let id = asset
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>");
        if let Some(period) = asset
            .get("loop")
            .and_then(|l| l.get("periodFrames"))
            .and_then(|v| v.as_u64())
        {
            // `expectedDurationFrames` is the same class of unbounded input as
            // `periodFrames`: `minimum: 1`, no maximum, and §8.4's residency is
            // `frames * 48000 * 2 * 4 / rate`, which panicked for a large
            // declaration. Saturating arithmetic keeps the report being
            // written; a duration whose residency has no addressable byte count
            // is refused by name rather than reported as a saturated number
            // nobody can act on.
            if period > ABSOLUTE_LOOP_FRAME_CAP {
                had_errors = true;
                report.push_error(format!(
                    "loopPeriod: asset \"{id}\" declares periodFrames {period}, beyond SPEC \
                     §12.4's absolute short-loop cap of {ABSOLUTE_LOOP_FRAME_CAP}"
                ));
            }

            // §12.5: "unless `cachePolicy: vram` is mandatory, in which case
            // preflight MUST fail". A package demanding residency the budget
            // cannot give does not quietly stream — it does not fit, and an
            // operator needs to hear that before air rather than discover it
            // as a different picture than the one they authored.
            let lm = asset.get("loop").expect("checked above");
            if lm.get("cachePolicy").and_then(|v| v.as_str()) == Some("vram") {
                let plan = loop_plan(asset, lm, &manifest_json, &report);
                if plan.cache_policy_selected != nbe_core::loop_cache::CachePolicy::Vram {
                    had_errors = true;
                    report.push_error(format!(
                        "loopBudget: asset \"{id}\" mandates cachePolicy \"vram\" but its \
                         {period} frames exceed the budget's {} (SPEC §12.5)",
                        plan.max_frames_by_budget
                    ));
                }
            }
        }

        if matches!(
            asset.get("kind").and_then(|v| v.as_str()),
            Some("audio") | Some("video") | Some("alphaVideo")
        ) {
            let frames = asset
                .get("expectedDurationFrames")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if frames.checked_mul(AUDIO_BYTES_PER_SECOND).is_none() {
                had_errors = true;
                report.push_error(format!(
                    "audioDuration: asset \"{id}\" declares expectedDurationFrames {frames}, \
                     whose §8.4 residency has no addressable byte count"
                ));
            }
        }
    }

    // SPEC §12.11 (v0.4): package demand, always reported.
    report.resources = resource_demand(&manifest_json, &report);
    // §7.15 #2: warn only when we were TOLD a target rate and it differs.
    // Warning merely because a package declares a rate would make every
    // package non-air-ready and teach operators to pass --allow-warnings by
    // reflex, which is how a warning stops meaning anything.
    if let (Some(declared), Some(target)) = (report.resources.declared_house_rate, house_rate) {
        if declared != target {
            report.push_warning(format!(
                "houseRate: package declares {declared} fps but the target runs at {target} fps; \
                 loading it there will mis-map every non-house-rate asset (SPEC §7.15)"
            ));
        }
    }

    // SPEC §7.10 / step 5 §3.4: overlay IDs are unique across the `overlays`
    // array, and overlay element references resolve against the same indexes a
    // scene element's do. The schema already constrains overlay element shape
    // (`Overlay` carries `Element[]`), so this is a reference check, not a
    // shape check: an `assetId` must name a declared asset, a `templateId` a
    // declared template, a `sceneRef` a declared scene.
    if let Some(overlays) = manifest_json.get("overlays").and_then(|o| o.as_array()) {
        let ids = |key: &str| -> HashSet<String> {
            manifest_json
                .get(key)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.get("id").and_then(|v| v.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        let asset_ids: HashSet<String> = ids("assets");
        let template_ids: HashSet<String> = ids("templates");
        let scene_ids: HashSet<String> = ids("scenes");

        // A template's fonts must resolve too: `fontAssetIds` lives on the
        // template, not the element, so it is checked against the template the
        // element names.
        let template_fonts: std::collections::HashMap<String, Vec<String>> = manifest_json
            .get("templates")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        let id = t.get("id").and_then(|v| v.as_str())?.to_string();
                        let fonts = t
                            .get("fontAssetIds")
                            .and_then(|f| f.as_array())
                            .map(|f| {
                                f.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some((id, fonts))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut seen = HashSet::new();
        for overlay in overlays {
            let id = overlay.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if !seen.insert(id.to_string()) {
                had_errors = true;
                report.push_error(format!(
                    "duplicateOverlay: id \"{id}\" appears more than once in the overlays array"
                ));
            }
            for el in overlay
                .get("elements")
                .and_then(|e| e.as_array())
                .into_iter()
                .flatten()
            {
                if let Some(aid) = el.get("assetId").and_then(|v| v.as_str()) {
                    if !asset_ids.contains(aid) {
                        had_errors = true;
                        report.push_error(format!(
                            "overlayAsset: overlay \"{id}\" references undeclared assetId \"{aid}\""
                        ));
                    }
                }
                if let Some(tid) = el.get("templateId").and_then(|v| v.as_str()) {
                    if !template_ids.contains(tid) {
                        had_errors = true;
                        report.push_error(format!(
                            "overlayTemplate: overlay \"{id}\" references undeclared templateId \"{tid}\""
                        ));
                    } else if let Some(fonts) = template_fonts.get(tid) {
                        for font in fonts {
                            if !asset_ids.contains(font) {
                                had_errors = true;
                                report.push_error(format!(
                                    "overlayFont: overlay \"{id}\" template \"{tid}\" references undeclared fontAssetId \"{font}\""
                                ));
                            }
                        }
                    }
                }
                if let Some(sid) = el.get("sceneRef").and_then(|v| v.as_str()) {
                    if !scene_ids.contains(sid) {
                        had_errors = true;
                        report.push_error(format!(
                            "overlaySceneRef: overlay \"{id}\" references undeclared sceneRef \"{sid}\""
                        ));
                    }
                }
            }
        }
    }

    // Prompt 08 work item 5 (companion mapping): control.bindings reference
    // the registered §16 command list, and a present trigger must carry its
    // kind-required fields. A missing trigger is allowed (triggerless
    // intent). Like the overlay block above, this is a reference check, not
    // a shape check: the schema already constrains binding shape.
    if let Some(bindings) = manifest_json
        .get("control")
        .and_then(|c| c.get("bindings"))
        .and_then(|b| b.as_array())
    {
        // Trigger rule (Prompt 08, WS-only): every present trigger names a
        // known kind and a non-empty key — the key is what a button, chord,
        // or note matches on. `companionKey` page/bank stay optional: an
        // omitted axis is a wildcard the matcher honours
        // (most-specific-wins, profile order breaks ties — documented on
        // `findCompanionEntry`, pinned by the control-plane suite). A missing
        // trigger is allowed: the intent is API-only and the deck generator
        // skips it (it cannot be button-fired). Identical triggers on two
        // bindings shadow silently, so duplicates fail by name.
        let mut seen_triggers = HashSet::new();
        for binding in bindings {
            let id = binding.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let action = binding.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let canonical = canonical_action(action);
            if canonical.is_none() {
                had_errors = true;
                report.push_error(format!(
                    "invalidBinding: binding \"{id}\" names unknown action \"{action}\" (E_PREFLIGHT_FAILED)"
                ));
            }
            // Absent payload counts as `{}`: required keys are required.
            match binding.get("payload") {
                None => {
                    for key in required_payload_keys(canonical.unwrap_or(action)) {
                        had_errors = true;
                        report.push_error(format!(
                            "invalidBinding: binding \"{id}\" action \"{action}\" payload is missing required key \"{key}\" (E_PREFLIGHT_FAILED)"
                        ));
                    }
                }
                Some(p) if !p.is_object() => {
                    had_errors = true;
                    report.push_error(format!(
                        "invalidBinding: binding \"{id}\" action \"{action}\" has non-object payload (E_PREFLIGHT_FAILED)"
                    ));
                }
                Some(p) => {
                    for key in required_payload_keys(canonical.unwrap_or(action)) {
                        if p.get(key).is_none() {
                            had_errors = true;
                            report.push_error(format!(
                                "invalidBinding: binding \"{id}\" action \"{action}\" payload is missing required key \"{key}\" (E_PREFLIGHT_FAILED)"
                            ));
                        }
                    }
                }
            }
            if let Some(trigger) = binding.get("trigger") {
                let kind = trigger.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                // Empty string is not a key: `Some("")` must not pass.
                let key = trigger
                    .get("key")
                    .and_then(|v| v.as_str())
                    .filter(|k| !k.is_empty());
                let known_kind =
                    ["companionKey", "hotkey", "midi", "webButton", "osc"].contains(&kind);
                let missing_field: Option<&str> = match kind {
                    "companionKey" | "hotkey" | "midi" | "webButton" | "osc" => {
                        if key.is_none() {
                            Some("key")
                        } else {
                            None
                        }
                    }
                    _ => {
                        had_errors = true;
                        report.push_error(format!(
                            "invalidBindingTrigger: binding \"{id}\" has unknown trigger kind \"{kind}\" (E_PREFLIGHT_FAILED)"
                        ));
                        None
                    }
                };
                if let Some(field) = missing_field {
                    had_errors = true;
                    report.push_error(format!(
                        "invalidBindingTrigger: binding \"{id}\" trigger kind \"{kind}\" is missing required field \"{field}\" (E_PREFLIGHT_FAILED)"
                    ));
                } else if known_kind {
                    let page = trigger.get("page").and_then(|v| v.as_u64());
                    let bank = trigger.get("bank").and_then(|v| v.as_u64());
                    let sig = format!(
                        "{}|{}|{}|{}",
                        kind,
                        page.map_or(String::from("-"), |p| p.to_string()),
                        bank.map_or(String::from("-"), |b| b.to_string()),
                        key.unwrap_or("-")
                    );
                    if !seen_triggers.insert(sig) {
                        had_errors = true;
                        report.push_error(format!(
                            "duplicateBindingTrigger: binding \"{id}\" trigger shadows another binding's identical trigger (E_PREFLIGHT_FAILED)"
                        ));
                    }
                }
            }
        }
    }

    report.finalize();
    Ok((report, had_errors))
}

/// SPEC §12.11.1: worst-case resident demand of a package.
///
/// Never a failure here: preflight cannot know what machine the package will
/// play on, and refusing on a guess is worse than reporting the number
/// (§12.11.3).
///
/// This READS the manifest rather than inferring from structure. The first
/// version inferred, and was wrong by 2.6x under, 13x over, 38x over, and
/// reported 0 MiB for an hour of audio: it ignored `textureFormat` and
/// `cachePolicy` — both in the schema, both stating the answer — charged every
/// image a full house-resolution frame, and sized loops by the show's
/// resolution instead of the asset's. That is precisely the defect §17.5
/// exists to abolish, committed in the same revision that wrote §17.5.
fn resource_demand(
    manifest: &serde_json::Value,
    report: &nbe_core::PreflightReport,
) -> nbe_core::ResourceReport {
    const MIB: u64 = 1024 * 1024;
    let video = manifest.get("show").and_then(|s| s.get("video"));
    let house_w = video
        .and_then(|v| v.get("width"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1920);
    let house_h = video
        .and_then(|v| v.get("height"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1080);
    let declared_house_rate = video
        .and_then(|v| v.get("frameRate"))
        .and_then(|v| v.as_u64())
        .map(|r| r as u32);

    // View + preview render targets and the fallback slate, RGBA8 at house
    // resolution. Unconditional.
    let mut vram = house_w
        .saturating_mul(house_h)
        .saturating_mul(4)
        .saturating_mul(3);

    for asset in manifest
        .get("assets")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
    {
        let id = asset.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let kind = asset.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        // One chain, shared with the loop planner: probed, else the house
        // frame.
        let (w, h) = asset_dimensions(id, manifest, report);

        match asset.get("loop") {
            Some(lm) => {
                let period = lm.get("periodFrames").and_then(|v| v.as_u64()).unwrap_or(0);

                // §12.2/§12.5: residency is a DECISION, not a declaration.
                // `cachePolicy: "auto"` means "let the budget decide", and the
                // first version honoured only the literal `"stream"` — so an
                // auto loop the budget mandates streaming for was charged as
                // resident (measured 2396 MiB where maxFrames = 32), and
                // `vramBudgetMib` was never read at all.
                //
                // The decision lives in `nbe_core::loop_cache::plan`, the same
                // function the engine uses to size its ring. One rule, one
                // implementation, across crates.
                let declared = lm
                    .get("cachePolicy")
                    .and_then(|v| v.as_str())
                    .unwrap_or("auto");
                let plan = loop_plan(asset, lm, manifest, report);
                let fits = plan.cache_policy_selected == nbe_core::loop_cache::CachePolicy::Vram;
                if declared == "stream" || !fits {
                    continue;
                }

                // The charge is read off the plan — format AND resolution —
                // rather than re-derived here. A second copy of the ladder is a
                // second place to be wrong about what a frame costs, and a
                // second copy of the dimensions is how the §12.5 gate and this
                // estimator came to disagree about the same loop.
                let per_frame = (plan.frame_cost_mib * MIB as f64) as u64;
                // Saturating, not wrapping. `periodFrames` has `minimum: 1`
                // and NO maximum in the schema, so a package may legally
                // declare a period near u64::MAX. That PANICKED preflight in
                // debug and wrapped in release — and a panic means no report
                // is written at all, which breaks P1's locked
                // report-on-every-run behaviour.
                vram = vram.saturating_add(period.saturating_mul(per_frame));
            }
            // §12.7: images upload whole, at their OWN size.
            None if kind == "image" => {
                vram = vram.saturating_add(w.saturating_mul(h).saturating_mul(4));
            }
            None => {}
        }
    }

    // §8.4 makes soundboard and clip audio RAM-resident from show.load, as
    // f32: 48 kHz x 2 ch x 4 B = 384 kB/s. An hour is 1.32 GiB.
    //
    // `expectedDurationFrames` is optional in the schema, so an audio asset may
    // declare no duration. Taking the larger of declared and probed means an
    // hour of audio is no longer reported as 0 MiB — the exact scenario §12.11
    // was written for.
    let rate = declared_house_rate.unwrap_or(30) as u64;
    let audio_bytes: u64 = manifest
        .get("assets")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter(|a| {
            matches!(
                a.get("kind").and_then(|v| v.as_str()),
                Some("audio") | Some("video") | Some("alphaVideo")
            )
        })
        .map(|a| {
            let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let declared = a
                .get("expectedDurationFrames")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let probed = report
                .assets
                .iter()
                .find(|x| x.id == id)
                .and_then(|x| x.duration_frames)
                .map(u64::from)
                .unwrap_or(0);
            declared.max(probed).saturating_mul(AUDIO_BYTES_PER_SECOND) / rate.max(1)
        })
        // `sum()` panics on overflow in debug just as `*` did: the saturation
        // has to survive the fold, not only each term.
        .fold(0u64, u64::saturating_add);

    nbe_core::ResourceReport {
        vram_demand_mib: vram / MIB,
        audio_demand_mib: audio_bytes / MIB,
        declared_house_rate,
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let (report, had_errors) = match run(&args.package_path, args.house_rate) {
        Ok(v) => v,
        Err(e) => {
            let mut report = PreflightReport::default();
            report.push_error(format!("{e:#}"));
            report.finalize();
            (report, true)
        }
    };

    // Write the report next to the manifest (SPEC 19.2).
    let report_path = args.package_path.join("preflight_report.json");
    match serde_json::to_string_pretty(&report) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&report_path, json) {
                eprintln!("error: failed to write {}: {e}", report_path.display());
                return ExitCode::from(2);
            }
        }
        Err(e) => {
            eprintln!("error: failed to serialize preflight report: {e}");
            return ExitCode::from(2);
        }
    }

    let has_warnings = !report.warnings.is_empty();
    if had_errors {
        eprintln!(
            "preflight FAILED ({} error(s), {} warning(s)): report at {}",
            report.errors.len(),
            report.warnings.len(),
            report_path.display()
        );
        ExitCode::from(2)
    } else if has_warnings && !args.allow_warnings {
        eprintln!(
            "preflight warnings only ({} warning(s)): not air-ready without --allow-warnings. report at {}",
            report.warnings.len(),
            report_path.display()
        );
        ExitCode::from(1)
    } else {
        println!(
            "preflight OK: air-ready. report at {}",
            report_path.display()
        );
        ExitCode::from(0)
    }
}
