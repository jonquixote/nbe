//! Telemetry (Prompt 03 Step 5, SPEC §10.1.1 [ownership]): the engine is
//! authoritative for clock/perf fields only; the show-state fields are the
//! control plane's. Emit an `engineTelemetry` frame at the cadence Step 5
//! sets.

use crate::state::EngineState;
use nbe_protocol::{EngineFrame, EngineTelemetry};

/// Build one engine-owned telemetry frame from the current state. The shape
/// must be complete even when values are stubs — a consumer should never see
/// a missing field (§5.9.1 + §10.1.1).
pub fn build_tick(state: &EngineState) -> EngineFrame {
    let frame = EngineTelemetry {
        master_clock_frame: state.master_frame().unwrap_or(0),
        dropped_frames_total: 0,
        render_gpu_time_ms: 0.0,
        decode_sessions: 0,
        vram_used_mib: 0.0,
        texture_cache_used_mib: 0.0,
        stream_buffer_ms: 0.0,
        record_space_mib: 0.0,
        master_clock_drift_ms: 0.0,
        fallback_active: state
            .fallback_active
            .load(std::sync::atomic::Ordering::SeqCst),
        degradation_rung: 0,
        // The effective profile comes from the wgpu adapter probe, which
        // arrives with the compositor in Prompt 04. Until then the control
        // plane reports the manifest's requested profile (SPEC §10.1.1).
        quality_profile: None,
    };
    EngineFrame::EngineTelemetry {
        v: nbe_protocol::PROTOCOL_VERSION.to_string(),
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0),
        fields: frame,
    }
}
