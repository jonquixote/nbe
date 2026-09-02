//! nbe-preflight: CI-runnable show-package validator.
//! SPEC Section 19: exit 0 air-ready, 1 warnings only, 2 errors.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use nbe_core::{AssetReport, PreflightReport, ValidationError};

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
            report.assets.push(AssetReport {
                id,
                kind,
                exists,
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
