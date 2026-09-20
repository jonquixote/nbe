//! ZERO-COPY Phase 2: the tap's selection, its fallback, and its reporting.
//!
//! Every test here drives a REAL entry point — `zerocopy::probe`,
//! `tap_path::select*`, `telemetry::build_tick` — never a hand-written state
//! effect. §2a rule 7 exists because that trap has cost this project three
//! passes, and a selection test that writes the selection it means to check
//! would be the fourth.

use nbe_engine::record::tap_path::{
    select, select_stream, select_with_override, Consumer, Reason, TapPath,
};
use nbe_engine::state::EngineState;

const HOUSE_RATE: u32 = 30;

/// The zero-copy chain needs a GPU. Where there is none, say so out loud rather
/// than passing quietly — the CI gate counts `exercised = ran - skipped`.
fn gpu_or_skip() -> Option<wgpu::Device> {
    let inst = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(inst.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let (device, _queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some(device)
}

// ---------------------------------------------------------------------------
// Falsification 1 — a failed probe falls back to CPU **and reports it**.
// ---------------------------------------------------------------------------

#[test]
fn a_failed_probe_selects_cpu_readback_and_the_fallback_reaches_telemetry() {
    // The selection comes from the real evaluator, not a literal.
    let selection = select(false, 1080, Consumer::Record);
    assert_eq!(selection.path, TapPath::CpuReadback);
    assert_eq!(
        selection.reason,
        Reason::ProbeUnavailable,
        "a fallback must say WHY, or an operator cannot tell it from a choice"
    );

    // And it must reach the wire. This drives `build_tick`, the real builder.
    let state = EngineState::new(HOUSE_RATE);
    *state.record_tap_selection.lock().unwrap() = Some(selection);
    let frame = nbe_engine::telemetry::build_tick(&state);
    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } = frame else {
        panic!("build_tick must produce telemetry");
    };
    assert_eq!(
        fields.record_tap_path, "cpuReadback",
        "the fallback path must be visible on the wire"
    );
    assert_eq!(
        fields.record_tap_reason, "ProbeUnavailable",
        "a silent fallback to the §0.1 assumption 24 allowance is the event this reports"
    );
}

/// **Replaces** `a_selection_that_was_never_made_reports_nothing_rather_than_a_default`
/// (ZERO-COPY Phase 2), which asserted the opposite: that the keys were ABSENT
/// until a take selected a path. That was a §10.1.1 violation shipped as a
/// guard — *"The emitted field shape is always complete. A telemetry consumer
/// MUST never see a missing field, whatever the engine's state — an absent
/// field and a stubbed field are different failures and only one of them is
/// diagnosable."* Found by the Phase 3a design memo (Q1′), fixed before the
/// migration because it is a defect in merged code independent of it.
///
/// The distinction the retired test was protecting is real and survives intact:
/// a machine that never recorded must not look like one that fell back. It is
/// now carried by `"none"` against `"cpuReadback"` — two values, both present,
/// rather than a key that is not there.
#[test]
fn the_tap_fields_are_always_on_the_wire_and_stub_before_any_take_selects() {
    let state = EngineState::new(HOUSE_RATE);
    let frame = nbe_engine::telemetry::build_tick(&state);

    // §10.1.1 COMPLETENESS. Asserted on the SERIALIZED frame, not the struct,
    // because "a consumer must never see a missing field" is a claim about the
    // wire — a `#[serde(skip_serializing)]` on the field would leave the struct
    // assertions below untouched and still strip the key from every tick.
    let wire = serde_json::to_value(&frame).expect("the tick serializes");
    let obj = wire
        .as_object()
        .expect("an engineTelemetry frame is an object");
    for key in ["recordTapPath", "recordTapReason"] {
        assert!(
            obj.contains_key(key),
            "§10.1.1: `{key}` must be present on every tick, before any take \
             selects a path; keys were {:?}",
            obj.keys().collect::<Vec<_>>()
        );
    }

    // And the value is the stub, which is NOT the fallback's value. This pair
    // is the whole distinction the retired test existed for.
    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } = frame else {
        panic!("telemetry");
    };
    assert_eq!(fields.record_tap_path, "none");
    assert_eq!(fields.record_tap_reason, "none");
    assert_ne!(
        fields.record_tap_path,
        TapPath::CpuReadback.as_str(),
        "a machine that never recorded must stay distinguishable from one that fell back"
    );
}

// ---------------------------------------------------------------------------
// Falsification 2 — a surface the chain cannot build refuses loudly.
// ---------------------------------------------------------------------------

#[test]
fn an_impossible_surface_refuses_loudly_rather_than_yielding_black_frames() {
    let Some(device) = gpu_or_skip() else {
        eprintln!("SKIP: no wgpu adapter on this machine; the zero-copy chain needs one");
        return;
    };
    // Drive the REAL probe with a geometry no IOSurface can have.
    let err = nbe_decode::zerocopy::probe(&device, 0, 1080)
        .expect_err("a zero-width surface must not succeed");
    let msg = err.to_string();
    assert!(
        msg.contains("E_NO_ZEROCOPY"),
        "the refusal must carry a stable token, got: {msg}"
    );
    assert!(
        msg.contains("implausible surface geometry"),
        "the refusal must name what was wrong, got: {msg}"
    );
}

#[test]
fn the_probe_builds_a_shared_surface_where_the_hardware_allows_it() {
    let Some(device) = gpu_or_skip() else {
        eprintln!("SKIP: no wgpu adapter on this machine; the zero-copy chain needs one");
        return;
    };
    let shared = nbe_decode::zerocopy::probe(&device, 1920, 1080)
        .expect("the chain Phase 1 proved must still build");
    assert_eq!(shared.dimensions(), (1920, 1080));
    assert_ne!(
        shared.surface_id(),
        0,
        "a real IOSurface has a non-zero id; this is what makes 'one allocation' checkable"
    );
    // The two views exist and are of that one surface.
    assert_eq!(shared.texture().width(), 1920);
    assert_eq!(shared.texture().format(), wgpu::TextureFormat::Bgra8Unorm);
}

// ---------------------------------------------------------------------------
// ZERO-COPY Phase 3b, step 2 — the device reaches the directive path.
// ---------------------------------------------------------------------------

/// `record.start` runs on the directive path and holds no wgpu handles. The
/// device lives in `RenderLoop`. This is the seam that lets the one ask the
/// other, and it is the `probed_quality` pattern §10.1.1 already mandates for
/// the sibling fact (`docs/zero-copy-p3-design.md`, Q1, option (a)).
///
/// Drives `RenderLoop::new` — the production constructor `main.rs:53` calls —
/// and never writes the publication itself (§2a rule 7).
#[tokio::test]
async fn the_render_loop_publishes_its_device_so_the_directive_path_can_probe() {
    let state = std::sync::Arc::new(EngineState::new(HOUSE_RATE));
    let Ok(_render) = nbe_engine::render::RenderLoop::new(state.clone()).await else {
        eprintln!("SKIP: no wgpu adapter on this machine; RenderLoop::new cannot open a device");
        return;
    };

    // Exactly the question step 5's `record.start` will ask, asked the way it
    // will ask it: through `EngineState`, with no device handle of its own.
    let capable = state
        .render_device()
        .map(|d| nbe_decode::zerocopy::is_available(&d, 1920, 1080))
        .unwrap_or(false);
    let selection = select(capable, 1080, Consumer::Record);

    assert_eq!(
        selection.reason,
        Reason::Table,
        "with the device published the table speaks for itself; drop the \
         publication and the directive path reports ProbeUnavailable on a \
         machine that has a working GPU — a fallback that is not a fallback"
    );
    assert_eq!(selection.path, TapPath::ZeroCopy);
    assert!(
        state.render_device().is_some(),
        "the handle must survive in state, not merely have existed during new()"
    );
}

// ---------------------------------------------------------------------------
// Falsification 3 — the published table drives the choice, not a pin.
// ---------------------------------------------------------------------------

#[test]
fn the_table_drives_the_choice_and_an_override_cannot_conjure_a_capability() {
    // Pin the probe "wrong" — claim zero-copy where the capability is absent.
    // The table must refuse it, because an override restricts and never invents.
    let pinned = select_with_override(false, 1080, Consumer::Record, Some(TapPath::ZeroCopy));
    assert_eq!(
        pinned.path,
        TapPath::CpuReadback,
        "an override to ZeroCopy on an incapable machine must not be honoured"
    );
    assert_eq!(pinned.reason, Reason::ProbeUnavailable);

    // The other direction is honoured, and says it was an override.
    let restricted = select_with_override(true, 2160, Consumer::Record, Some(TapPath::CpuReadback));
    assert_eq!(restricted.path, TapPath::CpuReadback);
    assert_eq!(restricted.reason, Reason::Override);

    // With no override the table speaks for itself.
    assert_eq!(
        select(true, 1080, Consumer::Record).reason,
        Reason::Table,
        "absent an override the choice is the table's, and says so"
    );
}

#[test]
fn streaming_has_no_lawful_readback_row() {
    // SPEC §0.1 assumption 24 + the v0.4.2 allowance, which is recording-only.
    // Prompt 10 must not arrive to find the tap defaulted to the path it is
    // forbidden from, so the incapable case is None rather than CpuReadback.
    assert!(
        select_stream(false).is_none(),
        "streaming without zero-copy has no path the spec permits"
    );
    assert_eq!(select_stream(true).unwrap().path, TapPath::ZeroCopy);
}
