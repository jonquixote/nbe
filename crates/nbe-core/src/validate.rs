//! Manifest validation: version gate + JSON Schema validation.
//! SPEC Sections 6.7 (migration), 15 (schema), Assumption 18, AC-28.

use thiserror::Error;

/// The byte-exact normative schema, embedded at compile time.
const SCHEMA_JSON: &str = include_str!("../../../schemas/manifest.v0.4.json");

/// Errors produced by manifest validation.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// The manifest's `manifestVersion` is neither `"0.3"` nor `"0.4"`.
    /// AC-28: a v0.2 package presented to a v0.3+ preflight MUST be rejected.
    #[error(
        "migration required: manifest has manifestVersion \"{found}\", expected \"0.3\" or \"0.4\". \
         Run `nbe-migrate` to convert this package."
    )]
    MigrationRequired { found: String },

    /// The manifest has no `manifestVersion` field at all. This is a
    /// malformed manifest, not a migration target.
    #[error("malformed manifest: no `manifestVersion` field present")]
    MissingVersion,

    /// The manifest failed JSON Schema validation.
    /// A transport SPEC §9 names but this revision does not implement. The
    /// schema refuses it as well; this carries the reason.
    #[error(
        "manifest declares stream protocol \"{protocol}\", which this build does not \
         implement: {reason}"
    )]
    RefusedTransport { protocol: String, reason: String },
    #[error("manifest schema validation failed:\n{details}")]
    SchemaViolation { details: String },

    /// The embedded schema itself could not be compiled (spec bug).
    #[error("internal error: embedded schema failed to compile: {0}")]
    SchemaCompile(String),
}

/// Return the compiled validator for the embedded schema, compiled once.
fn compiled_validator() -> Result<&'static jsonschema::Validator, ValidationError> {
    static VALIDATOR: std::sync::OnceLock<Result<jsonschema::Validator, String>> =
        std::sync::OnceLock::new();
    let result = VALIDATOR.get_or_init(|| {
        let schema: serde_json::Value =
            serde_json::from_str(SCHEMA_JSON).map_err(|e| e.to_string())?;
        jsonschema::validator_for(&schema).map_err(|e| e.to_string())
    });
    match result {
        Ok(v) => Ok(v),
        Err(e) => Err(ValidationError::SchemaCompile(e.clone())),
    }
}

/// Check only the version gate. Cheap; runs before schema validation.
///
/// SPEC v0.4 accepts `"0.3"` and `"0.4"`: the only schema change is the
/// removal of the `sequenceRef` hook, so a v0.3 manifest that never used it is
/// a valid v0.4 manifest. A manifest at any other version is a migration
/// target,
/// not a malformed manifest — it gets a dedicated error so callers can
/// give actionable guidance. A missing field is malformed instead.
pub fn check_version(json: &serde_json::Value) -> Result<(), ValidationError> {
    match json.get("manifestVersion").and_then(|v| v.as_str()) {
        None => Err(ValidationError::MissingVersion),
        Some("0.3") | Some("0.4") => Ok(()),
        Some(other) => Err(ValidationError::MigrationRequired {
            found: other.to_string(),
        }),
    }
}

/// Transports SPEC §9 names but this revision does not implement, each with the
/// reason it is deferred (SPEC v0.4.5, §9.1 and §9.4).
///
/// The schema refuses them too — `outputs.stream.protocol`'s enum is narrowed
/// to what is buildable — so this list exists for the MESSAGE, not for the
/// refusal. `"whip" is not one of "rtmp"` tells an operator what was rejected
/// and not why, and "why" is the difference between fixing the manifest and
/// filing a bug.
const DEFERRED_TRANSPORTS: &[(&str, &str)] = &[
    (
        "srt",
        "deferred pending a policy decision about the workspace's single \
         unsafe_code exemption: most SRT stacks are libsrt bindings (SPEC v0.4.5 §9.1)",
    ),
    (
        "whip",
        "a future contribution output, not a v1 streaming transport (SPEC §9.1 item 5)",
    ),
];

/// Name a refused transport before the schema's generic enum error does.
///
/// Runs before schema validation so the specific message wins. Returns `Ok` for
/// anything else, including a protocol nobody has ever named — that one is the
/// schema's to reject, and inventing a reason for it would be inventing a fact.
fn check_transport(json: &serde_json::Value) -> Result<(), ValidationError> {
    let declared = json
        .get("show")
        .and_then(|s| s.get("outputs"))
        .and_then(|o| o.get("stream"))
        .and_then(|s| s.get("protocol"))
        .and_then(|p| p.as_str());
    let Some(declared) = declared else {
        return Ok(());
    };
    for (name, reason) in DEFERRED_TRANSPORTS {
        if declared == *name {
            return Err(ValidationError::RefusedTransport {
                protocol: declared.to_string(),
                reason: (*reason).to_string(),
            });
        }
    }
    Ok(())
}

/// Validate a manifest value: version gate first, then JSON Schema.
pub fn validate_manifest(json: &serde_json::Value) -> Result<(), ValidationError> {
    check_version(json)?;
    check_transport(json)?;
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
    fn a_sequenceref_item_is_refused_at_validation() {
        // SPEC §16.4 (v0.4): the hook is retired, and "retired" has to mean
        // something a package can be measured against. The schema dropped
        // `sequenceRef` from `Item.kind`; nothing asserted that a manifest
        // using it is refused, so restoring the enum member would have gone
        // unnoticed — and `ItemKind` still carried a `SequenceRef` variant the
        // schema could no longer produce.
        let mut m = minimal_valid_manifest();
        // Only the retired `kind` — nothing else about this item is
        // irregular, so the enum is the only thing that can refuse it. With
        // `sequenceId` alongside, restoring the enum member still failed the
        // test, but on the unknown property rather than on the retirement:
        // a falsification that fails for the adjacent reason proves nothing.
        m["rundown"]["items"] = serde_json::json!([
            { "id": "A1", "kind": "sequenceRef" }
        ]);
        let err = validate_manifest(&m).expect_err("a retired hook must not validate");
        let text = err.to_string();
        assert!(
            text.contains("kind") || text.contains("sequenceRef"),
            "the refusal must point at the retired field; got {text}"
        );

        // And the retirement is scoped: a v0.3 manifest that never used the
        // hook is still a valid v0.4 manifest (§16.4's migration note).
        assert!(validate_manifest(&minimal_valid_manifest()).is_ok());
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
    fn missing_version_is_malformed_not_migration() {
        let mut m = minimal_valid_manifest();
        m.as_object_mut().unwrap().remove("manifestVersion");
        let err = validate_manifest(&m).unwrap_err();
        assert!(matches!(err, ValidationError::MissingVersion));
    }

    /// SPEC v0.4.5 §9.4: a manifest naming a transport this build does not
    /// implement fails VALIDATION — before load, before preflight's semantic
    /// checks, before any command.
    #[test]
    fn a_refused_transport_fails_validation_and_says_why() {
        for (proto, expect_in_reason) in [("whip", "contribution output"), ("srt", "libsrt")] {
            let mut m = minimal_valid_manifest();
            m["show"]["outputs"] = serde_json::json!({
                "stream": { "protocol": proto, "url": "rtmp://example.invalid/app/key" }
            });
            let err = validate_manifest(&m)
                .expect_err("a transport this build cannot speak must not validate");
            let msg = err.to_string();
            assert!(
                msg.contains(proto),
                "the refusal must name the protocol, got: {msg}"
            );
            assert!(
                msg.contains(expect_in_reason),
                "the refusal must carry the REASON, not just the rejection, got: {msg}"
            );
            assert!(
                matches!(err, ValidationError::RefusedTransport { .. }),
                "a refused transport is its own failure, not a generic schema violation"
            );
        }
    }

    /// The other half: the transport this build DOES implement validates, with
    /// the endpoint and the tap-path preference the revision added.
    #[test]
    fn an_rtmp_manifest_with_an_endpoint_validates() {
        let mut m = minimal_valid_manifest();
        m["show"]["outputs"] = serde_json::json!({
            "stream": {
                "protocol": "rtmp",
                "url": "rtmp://example.invalid/app/key",
                "videoBitrateKbps": 8000,
                "audioBitrateKbps": 192,
                "tapPath": "auto"
            },
            "record": { "directory": "./out", "tapPath": "cpuReadback" }
        });
        validate_manifest(&m).expect("the v1 transport with its endpoint must validate");
    }

    /// The narrowing has teeth at the SCHEMA level too, not only in the
    /// message: bypass `check_transport` by naming a protocol no list carries,
    /// and the enum still refuses it.
    #[test]
    fn the_schema_enum_refuses_an_unknown_transport_on_its_own() {
        let mut m = minimal_valid_manifest();
        m["show"]["outputs"] = serde_json::json!({ "stream": { "protocol": "hls" } });
        let err = validate_manifest(&m).expect_err("an unlisted protocol must not validate");
        assert!(
            matches!(err, ValidationError::SchemaViolation { .. }),
            "an unknown transport is the schema's to reject, got: {err}"
        );
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
