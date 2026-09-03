//! The mirror audit: this crate and the TypeScript control plane must agree,
//! and both must agree with the spec.
//!
//! Three layers, deliberately:
//!   1. Round-trip every frame through serde, so the Rust types produce the
//!      bytes the spec shows.
//!   2. Audit Rust against the SPEC v0.3.2 Section 16 tables (normative).
//!   3. Audit Rust against `packages/control-plane/src/protocol.ts` directly,
//!      so a divergence between the two implementations fails here even if
//!      both drift from the spec in the same direction.
//!
//! Layer 3 is what makes this crate a mirror rather than a second definition.

use nbe_protocol::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // crates/nbe-protocol -> repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn spec_text() -> String {
    std::fs::read_to_string(repo_root().join("docs/spec.v0.3.md")).expect("spec is readable")
}

fn section_16() -> String {
    let spec = spec_text();
    let start = spec.find("# 16. Command API").expect("Section 16 present");
    let end = spec
        .find("# 17. State machine")
        .expect("Section 17 present");
    spec[start..end].to_string()
}

/// Leading-cell backticked names from the Section 16 tables.
fn spec_commands() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in section_16().lines() {
        let line = line.trim();
        if !line.starts_with("| `") {
            continue;
        }
        let inner = &line[3..];
        if let Some(end) = inner.find('`') {
            let name = &inner[..end];
            // Skip the Section 16.0 authorization matrix, whose leading cells
            // are command *families* (`ticker.*`), not command names.
            if name.contains('.')
                && !name.contains('*')
                && name.starts_with(|c: char| c.is_ascii_lowercase())
            {
                out.insert(name.to_string());
            }
        }
    }
    out
}

fn spec_error_codes() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in section_16().lines() {
        let line = line.trim();
        if !line.starts_with("| `E_") {
            continue;
        }
        let inner = &line[3..];
        if let Some(end) = inner.find('`') {
            out.insert(inner[..end].to_string());
        }
    }
    out
}

fn ts_protocol() -> String {
    std::fs::read_to_string(repo_root().join("packages/control-plane/src/protocol.ts"))
        .expect("TypeScript protocol is readable")
}

// ---------------------------------------------------------------------------
// Layer 1: round-trips
// ---------------------------------------------------------------------------

fn round_trip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("serializes");
    let back: T = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(value, &back, "round-trip drifted: {json}");
}

#[test]
fn envelope_and_responses_round_trip() {
    round_trip(&Envelope {
        v: PROTOCOL_VERSION.into(),
        id: "0d9f5c6a-7b8a-4a61-9b0a-5f5a5c8d99ab".into(),
        command: "view.take".into(),
        payload: serde_json::json!({}),
        base_state_version: Some(412),
    });
    round_trip(&Envelope {
        v: PROTOCOL_VERSION.into(),
        id: "0d9f5c6a-7b8a-4a61-9b0a-5f5a5c8d99ab".into(),
        command: "system.status".into(),
        payload: serde_json::json!({}),
        base_state_version: None,
    });
    round_trip(&Response::Ok {
        v: PROTOCOL_VERSION.into(),
        request_id: "req-1".into(),
        state_version: 413,
        data: serde_json::json!({"item": "A1"}),
    });
    round_trip(&Response::Error {
        v: PROTOCOL_VERSION.into(),
        request_id: "req-1".into(),
        state_version: 412,
        error: ErrorBody {
            code: ErrorCode::ForbiddenState,
            message: "No preview item armed.".into(),
        },
    });
}

#[test]
fn envelope_matches_the_spec_5_4_example_byte_for_byte() {
    // The exact JSON from SPEC 5.4 must deserialize, and re-serialize to the
    // same field names. A rename drift here is a wire break.
    let spec_example = serde_json::json!({
        "v": "0.3",
        "id": "0d9f5c6a-7b8a-4a61-9b0a-5f5a5c8d99ab",
        "command": "view.take",
        "payload": {},
        "baseStateVersion": 412
    });
    let parsed: Envelope =
        serde_json::from_value(spec_example.clone()).expect("spec example parses");
    let reserialized = serde_json::to_value(&parsed).expect("serializes");
    assert_eq!(spec_example, reserialized);
}

#[test]
fn render_channel_frames_round_trip() {
    round_trip(&DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: 91,
        state_version: 413,
        command: "view.take".into(),
        target: serde_json::json!({"itemRef": "A1"}),
        payload: serde_json::json!({"transition": "cut"}),
    });
    round_trip(&EngineFrame::AppliedStateVersion {
        v: PROTOCOL_VERSION.into(),
        state_version: 413,
    });
    round_trip(&EngineFrame::ItemEvent {
        v: PROTOCOL_VERSION.into(),
        item_ref: "A1".into(),
        event: ItemEvent::End,
        detail: None,
    });
    round_trip(&EngineFrame::ResyncRequest {
        v: PROTOCOL_VERSION.into(),
        reason: ResyncReason::SeqGap,
    });
    round_trip(&EngineFrame::EngineTelemetry {
        v: PROTOCOL_VERSION.into(),
        ts: 1768000000000.0,
        fields: EngineTelemetry {
            master_clock_frame: 54000,
            dropped_frames_total: 0,
            render_gpu_time_ms: 4.7,
            decode_sessions: 4,
            vram_used_mib: 1830.0,
            texture_cache_used_mib: 512.0,
            stream_buffer_ms: 210.0,
            record_space_mib: 512000.0,
            master_clock_drift_ms: 0.2,
            fallback_active: false,
            degradation_rung: 0,
            quality_profile: Some(QualityProfile::Consumer),
        },
    });
    round_trip(&PushFrame::StateChange {
        v: PROTOCOL_VERSION.into(),
        state_version: 413,
        changed: vec!["viewItem".into()],
        state: serde_json::json!({}),
    });
}

#[test]
fn directive_frame_wire_names_match_the_control_plane() {
    // The control plane emits camelCase `stateVersion`; a Rust snake_case slip
    // would parse as a missing field at 3 a.m. on air.
    let frame = DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: 0,
        state_version: 7,
        command: command::RESYNC.into(),
        target: serde_json::json!({}),
        payload: serde_json::json!({}),
    };
    let value = serde_json::to_value(&frame).expect("serializes");
    let obj = value.as_object().expect("object");
    for key in [
        "v",
        "kind",
        "seq",
        "stateVersion",
        "command",
        "target",
        "payload",
    ] {
        assert!(obj.contains_key(key), "directive frame is missing `{key}`");
    }
    assert_eq!(obj["kind"], "directive");
}

#[test]
fn an_unknown_frame_kind_is_a_parse_error_not_a_silent_accept() {
    let bogus = serde_json::json!({ "v": "0.3", "kind": "somethingElse", "stateVersion": 1 });
    assert!(serde_json::from_value::<EngineFrame>(bogus.clone()).is_err());
    assert!(serde_json::from_value::<DirectiveFrame>(bogus).is_err());
}

// ---------------------------------------------------------------------------
// Layer 2: audit against the spec (normative)
// ---------------------------------------------------------------------------

#[test]
fn the_spec_tables_are_parseable() {
    // Guards the two audits below: a parse that silently matches nothing
    // would make them vacuous.
    assert!(
        spec_commands().len() >= 50,
        "parsed only {} commands from Section 16",
        spec_commands().len()
    );
    assert!(
        spec_error_codes().len() >= 15,
        "parsed only {} error codes",
        spec_error_codes().len()
    );
}

#[test]
fn error_registry_matches_the_spec_exactly() {
    let ours: BTreeSet<String> = ErrorCode::ALL
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    assert_eq!(ours, spec_error_codes());
}

#[test]
fn error_code_serialization_matches_as_str() {
    // `as_str` and the serde rename are two hand-written lists of the same
    // thing; keep them from diverging.
    for code in ErrorCode::ALL {
        let json = serde_json::to_string(code).expect("serializes");
        assert_eq!(json, format!("\"{}\"", code.as_str()));
    }
}

#[test]
fn command_surface_matches_the_spec_exactly() {
    let ours: BTreeSet<String> = command::ALL.iter().map(|c| c.to_string()).collect();
    assert_eq!(ours, spec_commands());
    // show.resync is directive-only and must NOT be a client command.
    assert!(!command::is_known(command::RESYNC));
}

// ---------------------------------------------------------------------------
// Layer 3: audit against the TypeScript control plane
// ---------------------------------------------------------------------------

#[test]
fn rust_and_typescript_agree_on_the_command_surface() {
    let ts = ts_protocol();
    // The zod registry keys: lines of the form `  "name.sub": z.object(...`
    let mut ts_commands = BTreeSet::new();
    for line in ts.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('"') {
            continue;
        }
        let Some(end) = trimmed[1..].find('"') else {
            continue;
        };
        let name = &trimmed[1..=end];
        if trimmed[end + 2..].trim_start().starts_with(':') && name.contains('.') {
            ts_commands.insert(name.to_string());
        }
    }
    let ours: BTreeSet<String> = command::ALL.iter().map(|c| c.to_string()).collect();
    assert_eq!(
        ours, ts_commands,
        "Rust and TypeScript command surfaces diverged"
    );
}

#[test]
fn rust_and_typescript_agree_on_the_error_registry() {
    let ts = ts_protocol();
    let start = ts
        .find("ErrorCodeSchema = z.enum([")
        .expect("error enum present");
    let end = ts[start..].find("]);").expect("enum terminates") + start;
    let mut ts_codes = BTreeSet::new();
    for chunk in ts[start..end].split('"') {
        if chunk.starts_with("E_") {
            ts_codes.insert(chunk.to_string());
        }
    }
    let ours: BTreeSet<String> = ErrorCode::ALL
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    assert_eq!(
        ours, ts_codes,
        "Rust and TypeScript error registries diverged"
    );
}

#[test]
fn rust_and_typescript_agree_on_the_engine_telemetry_fields() {
    // The frame-kind audit below does not look inside the frames. Telemetry is
    // the frame most likely to grow a field on one side only (Prompt 04 adds
    // qualityProfile; 05 will add decodeSessions values, 09/10 the output
    // fields), so audit its key set directly.
    let sample = EngineTelemetry {
        master_clock_frame: 0,
        dropped_frames_total: 0,
        render_gpu_time_ms: 0.0,
        decode_sessions: 0,
        vram_used_mib: 0.0,
        texture_cache_used_mib: 0.0,
        stream_buffer_ms: 0.0,
        record_space_mib: 0.0,
        master_clock_drift_ms: 0.0,
        fallback_active: false,
        degradation_rung: 0,
        quality_profile: Some(QualityProfile::Consumer),
    };
    let value = serde_json::to_value(&sample).expect("serializes");
    let ours: BTreeSet<String> = value.as_object().expect("object").keys().cloned().collect();

    let ts = ts_protocol();
    let start = ts
        .find("EngineTelemetryFrameSchema")
        .expect("engine telemetry schema present");
    let end = ts[start..].find(".strict()").expect("schema terminates") + start;
    let block = &ts[start..end];
    for field in &ours {
        assert!(
            block.contains(&format!("{field}:")),
            "TypeScript engineTelemetry has no `{field}` field"
        );
    }
}

#[test]
fn quality_profile_matches_the_manifest_schema_enum() {
    // SPEC §10.5 and schemas/manifest.v0.3.json must agree on the spelling.
    let schema = std::fs::read_to_string(repo_root().join("schemas/manifest.v0.3.json"))
        .expect("schema is readable");
    let schema: serde_json::Value = serde_json::from_str(&schema).expect("schema parses");
    let declared: BTreeSet<String> = schema["properties"]["qualityProfile"]["enum"]
        .as_array()
        .expect("qualityProfile enum present")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect();
    let ours: BTreeSet<String> = QualityProfile::ALL
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();
    assert_eq!(ours, declared);
}

#[test]
fn effective_quality_profile_never_exceeds_the_requested_one() {
    // SPEC §10.1.1: fast hardware does not promote a `consumer` show.
    assert_eq!(
        QualityProfile::Reference.capped_by(QualityProfile::Consumer),
        QualityProfile::Consumer
    );
    assert_eq!(
        QualityProfile::Potato.capped_by(QualityProfile::Reference),
        QualityProfile::Potato
    );
}

#[test]
fn rust_and_typescript_agree_on_the_engine_frame_kinds() {
    let ts = ts_protocol();
    for kind in EngineFrame::KINDS {
        assert!(
            ts.contains(&format!("z.literal(\"{kind}\")")),
            "TypeScript has no `{kind}` engine frame"
        );
    }
    // And nothing extra on the TypeScript side.
    let ts_kinds: BTreeSet<&str> = [
        "engineTelemetry",
        "appliedStateVersion",
        "itemEvent",
        "resyncRequest",
    ]
    .into_iter()
    .filter(|k| ts.contains(&format!("z.literal(\"{k}\")")))
    .collect();
    let ours: BTreeSet<&str> = EngineFrame::KINDS.iter().copied().collect();
    assert_eq!(ours, ts_kinds);
}
