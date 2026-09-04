//! nbe-protocol: the Rust mirror of the NBE wire protocol.
//!
//! Normative source: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Section 5.4 (command
//! envelope), 5.4.1 (server-push frames), 5.9 (the render channel), 16
//! (command API and error registry).
//!
//! This crate is a **mirror**, not a second definition. The TypeScript control
//! plane (`packages/control-plane/src/protocol.ts`) and this crate both
//! conform to the spec; `tests/mirror.rs` audits both against the spec tables
//! and against each other, so a command or error code added on one side and
//! forgotten on the other fails the build.
//!
//! Nothing here performs I/O or holds state. It is types and their JSON
//! representation, so the render node (`nbe-engine`) and any future Rust
//! client speak the same bytes the control plane emits.

use serde::{Deserialize, Serialize};

/// Protocol version carried by every frame (`v`).
pub const PROTOCOL_VERSION: &str = "0.3";

/// Default local endpoint path (SPEC 5.3).
pub const WS_PATH: &str = "/nbe/v0.3";

/// Default local port (SPEC 5.3).
pub const DEFAULT_PORT: u16 = 8462;

// ---------------------------------------------------------------------------
// Roles (SPEC 5.3)
// ---------------------------------------------------------------------------

/// Connection role. The token is authoritative; `X-NBE-Role` must match it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Monitor,
    Operator,
    Producer,
    Admin,
    Render,
}

// ---------------------------------------------------------------------------
// Error registry (SPEC Section 16)
// ---------------------------------------------------------------------------

/// The Section 16 error registry, complete. Every variant's serialized form is
/// the exact code string; `tests/mirror.rs` audits this against the spec table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "E_BAD_PAYLOAD")]
    BadPayload,
    #[serde(rename = "E_FORBIDDEN_STATE")]
    ForbiddenState,
    #[serde(rename = "E_NOT_FOUND")]
    NotFound,
    #[serde(rename = "E_ASSET_MISSING")]
    AssetMissing,
    #[serde(rename = "E_DECODE")]
    Decode,
    #[serde(rename = "E_ENGINE")]
    Engine,
    #[serde(rename = "E_VERSION_CONFLICT")]
    VersionConflict,
    #[serde(rename = "E_UNSUPPORTED")]
    Unsupported,
    #[serde(rename = "E_UNSUPPORTED_FEATURE")]
    UnsupportedFeature,
    #[serde(rename = "E_AUTH")]
    Auth,
    #[serde(rename = "E_NO_HARDWARE_ENCODER")]
    NoHardwareEncoder,
    #[serde(rename = "E_NETWORK")]
    Network,
    #[serde(rename = "E_PREFLIGHT_FAILED")]
    PreflightFailed,
    #[serde(rename = "E_AUDIO")]
    Audio,
    #[serde(rename = "E_DISK")]
    Disk,
    #[serde(rename = "E_TIMEOUT")]
    Timeout,
    #[serde(rename = "E_TURN")]
    Turn,
    #[serde(rename = "E_ICE")]
    Ice,
    /// New in SPEC v0.3.2: distinct from `ForbiddenState` on purpose — the
    /// command was well-formed and permitted, merely too frequent.
    #[serde(rename = "E_RATE_LIMITED")]
    RateLimited,
}

impl ErrorCode {
    /// Every code, in registry order. Used by the enum audit.
    pub const ALL: &'static [ErrorCode] = &[
        ErrorCode::BadPayload,
        ErrorCode::ForbiddenState,
        ErrorCode::NotFound,
        ErrorCode::AssetMissing,
        ErrorCode::Decode,
        ErrorCode::Engine,
        ErrorCode::VersionConflict,
        ErrorCode::Unsupported,
        ErrorCode::UnsupportedFeature,
        ErrorCode::Auth,
        ErrorCode::NoHardwareEncoder,
        ErrorCode::Network,
        ErrorCode::PreflightFailed,
        ErrorCode::Audio,
        ErrorCode::Disk,
        ErrorCode::Timeout,
        ErrorCode::Turn,
        ErrorCode::Ice,
        ErrorCode::RateLimited,
    ];

    /// The wire string, e.g. `"E_BAD_PAYLOAD"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::BadPayload => "E_BAD_PAYLOAD",
            ErrorCode::ForbiddenState => "E_FORBIDDEN_STATE",
            ErrorCode::NotFound => "E_NOT_FOUND",
            ErrorCode::AssetMissing => "E_ASSET_MISSING",
            ErrorCode::Decode => "E_DECODE",
            ErrorCode::Engine => "E_ENGINE",
            ErrorCode::VersionConflict => "E_VERSION_CONFLICT",
            ErrorCode::Unsupported => "E_UNSUPPORTED",
            ErrorCode::UnsupportedFeature => "E_UNSUPPORTED_FEATURE",
            ErrorCode::Auth => "E_AUTH",
            ErrorCode::NoHardwareEncoder => "E_NO_HARDWARE_ENCODER",
            ErrorCode::Network => "E_NETWORK",
            ErrorCode::PreflightFailed => "E_PREFLIGHT_FAILED",
            ErrorCode::Audio => "E_AUDIO",
            ErrorCode::Disk => "E_DISK",
            ErrorCode::Timeout => "E_TIMEOUT",
            ErrorCode::Turn => "E_TURN",
            ErrorCode::Ice => "E_ICE",
            ErrorCode::RateLimited => "E_RATE_LIMITED",
        }
    }
}

// ---------------------------------------------------------------------------
// Command names (SPEC Section 16)
// ---------------------------------------------------------------------------

/// Every Section 16 command name, plus the directive-only `show.resync`.
///
/// The render node matches on these rather than on free strings, so a
/// directive naming a command this build does not know is a parse error
/// instead of a silently ignored frame.
pub mod command {
    /// The complete Section 16 command surface, audited against the spec.
    pub const ALL: &[&str] = &[
        // 16.1 show
        "show.load",
        "show.preflight",
        "show.start",
        "show.stop",
        "show.unload",
        // 16.2 view/preview
        "preview.set",
        "view.take",
        "view.cut",
        "view.fallback",
        // 16.3 scene
        "scene.arm",
        "scene.apply",
        // 16.4 sequence/item
        "sequence.arm",
        "sequence.unarm",
        "item.arm",
        "item.unarm",
        "item.stop",
        "item.reset",
        // 16.5 element/graphic
        "element.toggle",
        "element.set",
        "graphic.show",
        "graphic.hide",
        "graphic.update",
        "breaking.show",
        "breaking.hide",
        // 16.6 overlay
        "overlay.show",
        "overlay.hide",
        // 16.7 ticker
        "ticker.setSource",
        "ticker.override",
        "ticker.clearOverride",
        "ticker.refreshRss",
        // 16.8 soundboard/audio
        "soundboard.play",
        "soundboard.stop",
        "soundboard.stopAll",
        "audio.bus.set",
        "audio.duck",
        "guest.mute",
        // 16.9 guest
        "guest.connect",
        "guest.disconnect",
        "guest.setLayout",
        "guest.placeholder",
        "guest.configureReturn",
        "guest.getTurn",
        // 16.10 automation
        "automation.enable",
        "automation.disable",
        "automation.hold",
        // 16.11 snapshot/marker
        "snapshot.save",
        "snapshot.recall",
        "marker.add",
        // 16.12 plugin
        "plugin.reload",
        // 16.13 clock
        "clock.configure",
        // 16.14 output
        "record.start",
        "record.stop",
        "stream.start",
        "stream.stop",
        // 16.15 system
        "system.status",
        "system.telemetry.subscribe",
        "system.telemetry.unsubscribe",
    ];

    /// SPEC §5.9.4: directive-only. No client may issue it, so it is
    /// deliberately absent from `ALL`.
    pub const RESYNC: &str = "show.resync";

    /// True when `name` is a Section 16 command.
    pub fn is_known(name: &str) -> bool {
        ALL.contains(&name)
    }
}

// ---------------------------------------------------------------------------
// Command envelope and responses (SPEC 5.4)
// ---------------------------------------------------------------------------

/// Client-to-server command envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub v: String,
    pub id: String,
    pub command: String,
    pub payload: serde_json::Value,
    #[serde(
        rename = "baseStateVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub base_state_version: Option<u64>,
}

/// Server response, success or error (SPEC 5.4). `status` discriminates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Response {
    Ok {
        v: String,
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "stateVersion")]
        state_version: u64,
        data: serde_json::Value,
    },
    Error {
        v: String,
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "stateVersion")]
        state_version: u64,
        error: ErrorBody,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Server-push frames (SPEC 5.4.1)
// ---------------------------------------------------------------------------

/// Frames the control plane initiates. Not responses: no `requestId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PushFrame {
    #[serde(rename = "stateChange")]
    StateChange {
        v: String,
        #[serde(rename = "stateVersion")]
        state_version: u64,
        changed: Vec<String>,
        state: serde_json::Value,
    },
    #[serde(rename = "telemetry")]
    Telemetry { v: String, data: serde_json::Value },
}

// ---------------------------------------------------------------------------
// The render channel (SPEC 5.9)
// ---------------------------------------------------------------------------

/// Control plane → render node (SPEC 5.9.1).
///
/// `seq` is per-connection and resets to 0 on every (re)connect: a fresh
/// connection starting at 0 means "joined here", never "lost N directives"
/// (SPEC 5.9.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectiveFrame {
    pub v: String,
    pub kind: DirectiveKind,
    pub seq: u64,
    #[serde(rename = "stateVersion")]
    pub state_version: u64,
    pub command: String,
    pub target: serde_json::Value,
    pub payload: serde_json::Value,
}

/// The single-variant tag of a directive frame; present so an unknown `kind`
/// fails to parse rather than being read as a directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectiveKind {
    #[serde(rename = "directive")]
    Directive,
}

/// The Section 10.1 fields the render node owns (SPEC 10.1.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineTelemetry {
    pub master_clock_frame: u64,
    pub dropped_frames_total: u64,
    pub render_gpu_time_ms: f64,
    pub decode_sessions: u32,
    pub vram_used_mib: f64,
    pub texture_cache_used_mib: f64,
    pub stream_buffer_ms: f64,
    pub record_space_mib: f64,
    pub master_clock_drift_ms: f64,
    pub fallback_active: bool,
    pub degradation_rung: u32,
    /// The **effective** profile from the startup probe, capped by the
    /// manifest's requested profile (SPEC §10.1.1, §10.5). `None` before the
    /// probe has run — the control plane then reports the requested profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_profile: Option<QualityProfile>,
    /// Audio callbacks the engine could not fill in time (SPEC §8.10). The
    /// audio equivalent of `dropped_frames_total`.
    #[serde(default)]
    pub audio_underruns_total: u64,
    /// Measured audio-to-master-clock drift in milliseconds (SPEC §8.9).
    #[serde(default)]
    pub audio_drift_ms: f64,
    /// Per-bus peak level in dBFS, so an operator can see which bus is hot
    /// without opening a meter bridge (SPEC §10.1).
    #[serde(default)]
    pub bus_peak_dbfs: std::collections::BTreeMap<String, f64>,
}

/// SPEC §10.5 quality profiles, in ascending capability order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityProfile {
    Potato,
    Consumer,
    Pro,
    Reference,
}

impl QualityProfile {
    /// Every profile, ascending. Audited against the manifest schema enum.
    pub const ALL: &'static [QualityProfile] = &[
        QualityProfile::Potato,
        QualityProfile::Consumer,
        QualityProfile::Pro,
        QualityProfile::Reference,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            QualityProfile::Potato => "potato",
            QualityProfile::Consumer => "consumer",
            QualityProfile::Pro => "pro",
            QualityProfile::Reference => "reference",
        }
    }

    /// The effective profile never exceeds what the show requested
    /// (SPEC §10.1.1): fast hardware does not promote a `consumer` show.
    pub fn capped_by(self, requested: QualityProfile) -> QualityProfile {
        self.min(requested)
    }
}

/// Why the engine is asking for a snapshot (SPEC 5.9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResyncReason {
    SeqGap,
    Reconnect,
    Internal,
}

/// Item lifecycle events only the engine can observe (SPEC 5.9.3). Without
/// these, the `PLAYING -> DONE` and `-> MISSING`/`ERROR` rows of Section 17.3
/// are unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemEvent {
    End,
    DecodeError,
    DeviceLoss,
    Missing,
}

/// Render node → control plane (SPEC 5.9.3). Accepted only from `render`-role
/// sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum EngineFrame {
    #[serde(rename = "engineTelemetry")]
    EngineTelemetry {
        v: String,
        ts: f64,
        #[serde(flatten)]
        fields: EngineTelemetry,
    },
    #[serde(rename = "appliedStateVersion")]
    AppliedStateVersion {
        v: String,
        #[serde(rename = "stateVersion")]
        state_version: u64,
    },
    #[serde(rename = "itemEvent")]
    ItemEvent {
        v: String,
        #[serde(rename = "itemRef")]
        item_ref: String,
        event: ItemEvent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    #[serde(rename = "resyncRequest")]
    ResyncRequest { v: String, reason: ResyncReason },
}

impl EngineFrame {
    /// Every frame kind, for the enum audit.
    pub const KINDS: &'static [&'static str] = &[
        "engineTelemetry",
        "appliedStateVersion",
        "itemEvent",
        "resyncRequest",
    ];
}
