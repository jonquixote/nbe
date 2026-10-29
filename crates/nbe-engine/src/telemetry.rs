//! Telemetry (Prompt 03 Step 5, SPEC §10.1.1 [ownership]): the engine is
//! authoritative for clock/perf fields only; the show-state fields are the
//! control plane's. Emit an `engineTelemetry` frame at the cadence Step 5
//! sets.

use crate::state::EngineState;
use nbe_protocol::{EngineFrame, EngineTelemetry};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

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

/// How often [`RecordSpace`] measures the record target: the TTL
/// `available_space_mib` already caches a measurement for.
const SPACE_REFRESH: Duration = Duration::from_secs(5);

/// `recordSpaceMib`, measured off the async runtime (R36, 2026-10-28).
///
/// The telemetry tick runs on a tokio worker, and measuring free space forked
/// `df`. While a take recorded, that fork blocked the worker for one to two
/// seconds, and the render loop's sleep — a tokio timer the workers service —
/// ended that late: 28–63 master frames never drawn, about once per ten
/// seconds, on both GPUs (R33's measurement session; the fork removed, the
/// stalls went with it). A dedicated thread now measures the target the tick
/// last asked about, at once when the target changes and every
/// [`SPACE_REFRESH`] after; the tick reads its latest result and never waits
/// on a measurement. The measurement itself is now one statvfs-style call, no
/// process (`record::available_space_mib`); the thread stays, so the tick
/// never waits on any filesystem query.
pub struct RecordSpace {
    shared: Arc<(Mutex<SpaceSlot>, Condvar)>,
    probe: fn(&Path) -> f64,
}

#[derive(Default)]
struct SpaceSlot {
    /// The target the tick last asked about.
    want: Option<PathBuf>,
    /// The latest measurement, and the target it measured.
    have: Option<(PathBuf, f64)>,
    running: bool,
}

impl Default for RecordSpace {
    fn default() -> Self {
        Self::with_probe(|dir| record_space_mib_for(Some(dir)))
    }
}

impl RecordSpace {
    /// A measurer whose measurement is `probe` (the test seam: a slow volume).
    pub fn with_probe(probe: fn(&Path) -> f64) -> Self {
        Self {
            shared: Arc::new((Mutex::new(SpaceSlot::default()), Condvar::new())),
            probe,
        }
    }

    /// The latest free space measured on `dir`'s volume: `0.0` with no target,
    /// and until the first measurement of a new target lands.
    pub fn read(&self, dir: Option<&Path>) -> f64 {
        let Some(dir) = dir else { return 0.0 };
        let (slot, wake) = &*self.shared;
        let mut s = slot.lock().unwrap();
        if s.want.as_deref() != Some(dir) {
            s.want = Some(dir.to_path_buf());
            wake.notify_one();
        }
        if !s.running {
            s.running = true;
            let shared = Arc::downgrade(&self.shared);
            let probe = self.probe;
            std::thread::Builder::new()
                .name("nbe-record-space".into())
                .spawn(move || measure(shared, probe))
                .expect("spawn the record-space thread");
        }
        match &s.have {
            Some((measured, mib)) if measured == dir => *mib,
            _ => 0.0,
        }
    }
}

/// [`RecordSpace`]'s thread: measure the wanted target without holding the
/// slot, publish, then wait for the refresh or a new target. It ends with its
/// `RecordSpace`.
fn measure(shared: Weak<(Mutex<SpaceSlot>, Condvar)>, probe: fn(&Path) -> f64) {
    loop {
        let Some(want) = shared.upgrade().map(|s| s.0.lock().unwrap().want.clone()) else {
            return;
        };
        let measured = want.as_deref().map(probe);
        let Some(strong) = shared.upgrade() else {
            return;
        };
        let (slot, wake) = &*strong;
        let mut s = slot.lock().unwrap();
        if let (Some(dir), Some(mib)) = (&want, measured) {
            s.have = Some((dir.clone(), mib));
        }
        let _unchanged = wake
            .wait_timeout_while(s, SPACE_REFRESH, |s| s.want == want)
            .unwrap();
    }
}

/// Build one engine-owned telemetry frame, reading `recordSpaceMib` for the
/// target volume when `record_dir` is `Some` (measured off the runtime by
/// [`RecordSpace`]; the tick never waits on it).
pub fn build_tick_for_dir(state: &EngineState, record_dir: Option<&Path>) -> EngineFrame {
    // Read the tap selection ONCE, before the struct literal. Locking twice
    // inside it deadlocks: struct-literal temporaries live until the end of the
    // enclosing statement, so the first `MutexGuard` is still held when the
    // second `lock()` runs, on a non-reentrant `Mutex`. Found by
    // `pump_tick_wires_the_loaded_record_dir` hanging rather than failing.
    let tap = *state.record_tap_selection.lock().unwrap();
    // streamBufferMs (§10.1): the live session's admitted-but-unwritten bytes
    // through the stream's envelope bitrate (channel backlog INCLUDED — see
    // the rtmp module docs). With no session: -1.0, the NO-SESSION sentinel
    // (`nbe_protocol::STREAM_BUFFER_NO_SESSION_MS`), **ratified in v0.4.6** —
    // "no measurement exists", distinguishable from a live session's honest
    // 0.0, "the buffer is empty". `streamState` on the same tick carries idle
    // vs live, but it is commanded, not measured; the sentinel lets this
    // field be read alone.
    //
    // History kept per §2c. PR #30 first shipped -1.0 inside the feature PR;
    // the repair round reverted it (`f8ff895`) because changing a ratified
    // field's meaning is the user's change, not a feature PR's:
    //
    // > With no session the buffer holds nothing, and 0.0 says exactly that —
    // > a measurement of an absent buffer, not a stub standing in for one.
    // > Whether the stream is idle or drained-live is `streamState`'s job (the
    // > control plane's, "as commanded"), already on the same tick.
    //
    // The user made that change in v0.4.6, so this reverts the revert.
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
                .unwrap_or(nbe_protocol::STREAM_BUFFER_NO_SESSION_MS),
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
        record_space_mib: state.record_space.read(record_dir),
        master_clock_drift_ms: 0.0,
        fallback_active: state.fallback_active(),
        degradation_rung: state.degradation_rung(),
        // SPEC §10.3 (v0.4.7): the watchdog's fault counter, and its
        // recoveries — on the tick beside `degradationRung`, §10.5's
        // reporting shape for the ladder (Prompt 11 WU8).
        watchdog_trips_total: state
            .watchdog_trips_total
            .load(std::sync::atomic::Ordering::SeqCst),
        watchdog_clears_total: state
            .watchdog_clears_total
            .load(std::sync::atomic::Ordering::SeqCst),
        // SPEC §12.8.1 rule 4, §10.1 (v0.4.8 row 12): the View's clip frames
        // held because the read-ahead had not made them resident.
        read_ahead_misses_total: state
            .read_ahead_misses_total
            .load(std::sync::atomic::Ordering::SeqCst),
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
        // The device-audio design record's D5 and D6: which device carries
        // program audio, and the limiter's ceiling in force (D5's amendment).
        audio_device: state.audio_device.lock().unwrap().clone(),
        audio_limiter_ceiling: state.audio_limiter_ceiling.lock().unwrap().clone(),
    };
    EngineFrame::EngineTelemetry {
        v: nbe_protocol::PROTOCOL_VERSION.to_string(),
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0),
        fields: Box::new(frame),
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
