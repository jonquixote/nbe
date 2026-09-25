//! Telemetry (Prompt 03 Step 5, SPEC §10.1.1 [ownership]): the engine is
//! authoritative for clock/perf fields only; the show-state fields are the
//! control plane's. Emit an `engineTelemetry` frame at the cadence Step 5
//! sets.

use crate::state::EngineState;
use nbe_protocol::{EngineFrame, EngineTelemetry};
use std::path::Path;

/// Free space on the record target's volume, in MiB (SPEC §10.1).
///
/// Computed on the telemetry tick (or lazily by the caller holding the record
/// directory) — never in the frame path, so the render loop keeps its
/// deadline behavior unchanged. Unwritable/missing targets degrade to `0.0`
/// here; the loud `E_DISK` refusal lives in
/// [`crate::record::available_space_mib`], which this wraps.
pub fn record_space_mib_for(dir: Option<&Path>) -> f64 {
    dir.and_then(|d| crate::record::available_space_mib(d).ok())
        .unwrap_or(0.0)
}

/// Build one engine-owned telemetry frame, measuring `recordSpaceMib`
/// against the real target volume when `record_dir` is `Some`.
pub fn build_tick_for_dir(state: &EngineState, record_dir: Option<&Path>) -> EngineFrame {
    // Read the tap selection ONCE, before the struct literal. Locking twice
    // inside it deadlocks: struct-literal temporaries live until the end of the
    // enclosing statement, so the first `MutexGuard` is still held when the
    // second `lock()` runs, on a non-reentrant `Mutex`. Found by
    // `pump_tick_wires_the_loaded_record_dir` hanging rather than failing.
    let tap = *state.record_tap_selection.lock().unwrap();
    // streamBufferMs (§10.1, law): the live session's admitted-but-unwritten
    // bytes through the stream's envelope bitrate (channel backlog INCLUDED —
    // see the rtmp module docs). With no session the buffer holds nothing,
    // and 0.0 says exactly that — a measurement of an absent buffer, not a
    // stub standing in for one. Whether the stream is idle or drained-live is
    // `streamState`'s job (the control plane's, "as commanded"), already on
    // the same tick.
    //
    // PR #30 first shipped -1.0 here as a "NO-SESSION sentinel". That changed
    // the meaning of a ratified field inside a feature PR, which is the
    // user's change to make, not ours; it is reverted and drafted as an
    // UNRATIFIED candidate in `docs/v0.5-outline.md` §7 instead.
    //
    // Read under one short lock; the counter itself is atomic, so the tick
    // never waits on the socket. `streamTransportState` (§10.1, v0.4.6) is
    // read in the same scope: the publisher's own view of the socket, which
    // `streamState` ("as commanded") deliberately does not follow (§9.5).
    let (stream_buffer_ms, transport) = {
        let session = state.stream_session.lock().unwrap();
        (
            session
                .as_ref()
                .map(|s| s.stream_buffer_ms())
                .unwrap_or(0.0),
            session.as_ref().map(|s| s.publisher_state()),
        )
    };
    // No session: `"closed"` once any stream has opened (the socket is gone),
    // the `"none"` stub before one ever has — §10.1.1, the `recordTapPath`
    // precedent. `"none"` is not a transport state, so never-streamed stays
    // distinguishable from closed.
    let stream_transport_state = match transport {
        Some(p) => p.as_str().to_string(),
        None if state
            .stream_transport_opened
            .load(std::sync::atomic::Ordering::SeqCst) =>
        {
            crate::record::rtmp::PublisherState::Closed
                .as_str()
                .to_string()
        }
        None => nbe_protocol::tap_none(),
    };
    let frame = EngineTelemetry {
        master_clock_frame: state.master_frame().unwrap_or(0),
        dropped_frames_total: state
            .dropped_frames_total
            .load(std::sync::atomic::Ordering::SeqCst),
        render_gpu_time_ms: 0.0,
        decode_sessions: state.sessions.active(),
        vram_used_mib: 0.0,
        texture_cache_used_mib: 0.0,
        stream_buffer_ms,
        record_space_mib: record_space_mib_for(record_dir),
        master_clock_drift_ms: 0.0,
        fallback_active: state
            .fallback_active
            .load(std::sync::atomic::Ordering::SeqCst),
        degradation_rung: state.degradation_rung(),
        // Effective probe result from GPU init, capped by the manifest's
        // requested profile. The engine is the engine — it is authoritative.
        quality_profile: *state.quality_profile.lock().unwrap(),
        // SPEC §8.10 / §8.9 / §10.1: measured by the audio graph, published
        // here. Zero means "no audio engine running", not "no problem".
        audio_underruns_total: state
            .audio_underruns_total
            .load(std::sync::atomic::Ordering::SeqCst),
        audio_drift_ms: f64::from_bits(
            state
                .audio_drift_ms_bits
                .load(std::sync::atomic::Ordering::SeqCst),
        ),
        // A snapshot: the driver closes each meter window on its own boundary,
        // so reading here neither ends a window nor races another reader.
        bus_peak_dbfs: state.bus_peaks.lock().unwrap().clone(),
        // ZERO-COPY Phase 2: which path the record tap took, and why. Absent
        // until a take selects one — an operator reading this can tell a
        // machine that chose zero-copy from one that fell back to the §0.1
        // assumption 24 allowance, which is the whole reason it is reported.
        // §10.1.1: always emitted. `"none"` before a take selects — a stub, not
        // an omission, because an absent field and a stubbed field are
        // different failures and only one of them is diagnosable.
        record_tap_path: tap
            .map(|s| s.path.as_str().to_string())
            .unwrap_or_else(nbe_protocol::tap_none),
        record_tap_reason: tap
            .map(|s| format!("{:?}", s.reason))
            .unwrap_or_else(nbe_protocol::tap_none),
        stream_transport_state,
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

/// Build one engine-owned telemetry frame from the current state. The shape
/// must be complete even when values are stubs — a consumer should never see
/// a missing field (§5.9.1 + §10.1.1).
///
/// Render-loop path: unchanged. No record target is known here, so
/// `recordSpaceMib` reports `0.0`; callers holding the record directory use
/// [`build_tick_for_dir`]. The channel pump reads the loaded package's record
/// directory out of engine state and calls [`build_tick_for_dir`] with it.
pub fn build_tick(state: &EngineState) -> EngineFrame {
    build_tick_for_dir(state, None)
}
