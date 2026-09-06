//! nbe-preflight: CI-runnable show-package validator.
//! SPEC Section 19: exit 0 air-ready, 1 warnings only, 2 errors.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use nbe_core::{AssetReport, PreflightReport, ValidationError};

/// How many frames a preflight decode pass will read per asset. Preflight is
/// allowed to be slow; it is not allowed to be unbounded.
const DECODE_FRAME_LIMIT: usize = 100_000;

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
        let offending: &[&str] = match kind {
            "slate" => &["sceneRef", "assetId"],
            "sceneRef" => &["assetId"],
            _ => &[],
        };
        for field in offending {
            if item.get(*field).is_some() {
                had_errors = true;
                report.push_error(format!(
                    "contradictoryItem: item \"{id}\" is kind \"{kind}\" but carries \"{field}\" (SPEC §17.5)"
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

    report.finalize();
    Ok((report, had_errors))
}

/// SPEC §12.11.1: worst-case resident demand of a package.
///
/// Never a failure here: preflight cannot know what machine the package will
/// play on, and refusing on a guess is worse than reporting the number
/// (§12.11.3 #3).
fn resource_demand(
    manifest: &serde_json::Value,
    report: &nbe_core::PreflightReport,
) -> nbe_core::ResourceReport {
    const MIB: u64 = 1024 * 1024;
    let video = manifest.get("show").and_then(|s| s.get("video"));
    let width = video
        .and_then(|v| v.get("width"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1920);
    let height = video
        .and_then(|v| v.get("height"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1080);
    let declared_house_rate = video
        .and_then(|v| v.get("frameRate"))
        .and_then(|v| v.as_u64())
        .map(|r| r as u32);

    // View + preview render targets and the fallback slate, RGBA8 at house
    // resolution.
    let frame_rgba = width * height * 4;
    let mut vram = frame_rgba * 3;

    for asset in manifest
        .get("assets")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
    {
        let id = asset.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let kind = asset.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(period) = asset
            .get("loop")
            .and_then(|l| l.get("periodFrames"))
            .and_then(|v| v.as_u64())
        {
            // NV12 is 1.5 bytes/px; alphaVideo needs the full RGBA8.
            let per_frame = if kind == "alphaVideo" {
                frame_rgba
            } else {
                width * height * 3 / 2
            };
            vram += period * per_frame;
        } else if kind == "image" {
            let probed = report.assets.iter().find(|a| a.id == id);
            let w = probed.and_then(|a| a.width).map(u64::from).unwrap_or(width);
            let h = probed
                .and_then(|a| a.height)
                .map(u64::from)
                .unwrap_or(height);
            vram += w * h * 4;
        }
    }

    // §8.4 makes soundboard and clip audio RAM-resident from show.load, as
    // f32: 48 kHz x 2 ch x 4 B = 384 kB/s. An hour is 1.32 GiB.
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
            let frames = a
                .get("expectedDurationFrames")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            (frames * 48_000 * 2 * 4) / rate.max(1)
        })
        .sum();

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
