//! Manifest validation: version gate + JSON Schema validation.
//! SPEC Sections 6.7 (migration), 15 (schema), Assumption 18, AC-28.

use thiserror::Error;

/// The byte-exact normative schema, embedded at compile time.
const SCHEMA_JSON: &str = include_str!("../../../schemas/manifest.v0.3.json");

/// Errors produced by manifest validation.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// The manifest's `manifestVersion` is not `"0.3"`.
    /// AC-28: a v0.2 package presented to a v0.3 preflight MUST be rejected.
    #[error(
        "migration required: manifest has manifestVersion \"{found}\", expected \"0.3\". \
         Run `nbe-migrate` to convert this package to v0.3."
    )]
    MigrationRequired { found: String },

    /// The manifest failed JSON Schema validation.
    #[error("manifest schema validation failed:\n{details}")]
    SchemaViolation { details: String },

    /// The embedded schema itself could not be compiled (spec bug).
    #[error("internal error: embedded schema failed to compile: {0}")]
    SchemaCompile(String),
}

/// Return a compiled validator for the embedded schema.
fn compiled_validator() -> Result<jsonschema::Validator, ValidationError> {
    let schema: serde_json::Value = serde_json::from_str(SCHEMA_JSON)
        .map_err(|e| ValidationError::SchemaCompile(e.to_string()))?;
    jsonschema::validator_for(&schema).map_err(|e| ValidationError::SchemaCompile(e.to_string()))
}

/// Check only the version gate. Cheap; runs before schema validation.
///
/// A manifest whose `manifestVersion` is not `"0.3"` is a migration target,
/// not a malformed manifest — it gets a dedicated error so callers can
/// give actionable guidance.
pub fn check_version(json: &serde_json::Value) -> Result<(), ValidationError> {
    let found = json
        .get("manifestVersion")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if found != "0.3" {
        return Err(ValidationError::MigrationRequired {
            found: found.to_string(),
        });
    }
    Ok(())
}

/// Validate a manifest value: version gate first, then JSON Schema.
pub fn validate_manifest(json: &serde_json::Value) -> Result<(), ValidationError> {
    check_version(json)?;
    let validator = compiled_validator()?;
    let errors: Vec<String> = validator
        .iter_errors(json)
        .map(|e| {
            let path = e.instance_path().to_string();
            if path.is_empty() {
                e.to_string()
            } else {
                format!("{}: {}", path, e)
            }
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ValidationError::SchemaViolation {
            details: errors.join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_valid_manifest() -> serde_json::Value {
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "Test Network" },
            "show": {
                "id": "show",
                "title": "Test Show",
                "video": {
                    "width": 1920,
                    "height": 1080,
                    "frameRate": 30,
                    "colorSpace": "rec709"
                },
                "audio": {
                    "sampleRate": 48000,
                    "loudnessTargetLufs": -16,
                    "truePeakDbtp": -1.5
                },
                "fallbackAssetId": "fallback"
            },
            "assets": [
                { "id": "fallback", "kind": "image", "source": "media/fallback.png" }
            ],
            "scenes": [
                { "id": "SCN_A", "elements": [] }
            ],
            "rundown": {
                "id": "R",
                "items": [
                    { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_A" }
                ]
            },
            "control": { "bindings": [] }
        })
    }

    #[test]
    fn valid_v03_manifest_passes() {
        let m = minimal_valid_manifest();
        assert!(validate_manifest(&m).is_ok());
    }

    #[test]
    fn v02_manifest_yields_migration_required() {
        let mut m = minimal_valid_manifest();
        m["manifestVersion"] = serde_json::json!("0.2");
        let err = validate_manifest(&m).unwrap_err();
        match err {
            ValidationError::MigrationRequired { found } => assert_eq!(found, "0.2"),
            other => panic!("expected MigrationRequired, got: {other:?}"),
        }
    }

    #[test]
    fn missing_version_yields_migration_required() {
        let mut m = minimal_valid_manifest();
        m.as_object_mut().unwrap().remove("manifestVersion");
        let err = validate_manifest(&m).unwrap_err();
        assert!(matches!(err, ValidationError::MigrationRequired { .. }));
    }

    #[test]
    fn schema_violation_reported() {
        let mut m = minimal_valid_manifest();
        m.as_object_mut().unwrap().remove("network");
        let err = validate_manifest(&m).unwrap_err();
        assert!(matches!(err, ValidationError::SchemaViolation { .. }));
    }

    #[test]
    fn error_message_mentions_nbe_migrate() {
        let mut m = minimal_valid_manifest();
        m["manifestVersion"] = serde_json::json!("0.2");
        let err = validate_manifest(&m).unwrap_err();
        assert!(err.to_string().contains("nbe-migrate"));
    }
}
