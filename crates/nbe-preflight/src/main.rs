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
}

fn run(package_path: &Path) -> Result<(PreflightReport, bool)> {
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

    report.finalize();
    Ok((report, had_errors))
}

fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let (report, had_errors) = match run(&args.package_path) {
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
