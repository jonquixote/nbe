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
// ZERO-COPY Phase 3b, step 3 — the surface pool and the pixel-buffer encode.
// ---------------------------------------------------------------------------

/// Pool size: one surface in flight per channel slot, plus the one being drawn
/// into. Read from the production constant so the two cannot drift.
fn pool_size() -> usize {
    nbe_engine::record::thread::RECORD_CHANNEL_BOUND + 1
}

#[test]
fn an_exhausted_pool_skips_the_frame_before_the_draw_and_counts_it() {
    let Some(device) = gpu_or_skip() else {
        eprintln!("SKIP: no wgpu adapter on this machine; the zero-copy chain needs one");
        return;
    };
    let pool = nbe_decode::zerocopy::SurfacePool::new(&device, 1920, 1080, pool_size())
        .expect("the pool is built from the same probe Phase 1 proved");
    assert_eq!(pool.len(), pool_size());
    assert_eq!(pool.free(), pool_size(), "a fresh pool is entirely free");

    let skipped = std::sync::atomic::AtomicU64::new(0);

    // Hold every surface, as the encoder would while it works through a full
    // channel plus the frame in hand.
    let mut in_flight = Vec::new();
    for _ in 0..pool_size() {
        in_flight.push(
            nbe_engine::record::feed::acquire_record_surface(&pool, &skipped)
                .expect("a free surface while any remain"),
        );
    }
    assert_eq!(pool.free(), 0);
    assert_eq!(
        skipped.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "handing out surfaces that existed is not a skip"
    );

    // The frame that finds no surface. This is the whole point: the answer is
    // `None` and the count moves BEFORE anything is drawn — there is no
    // surface to draw into, so "shed after drawing" is not reachable.
    let denied = nbe_engine::record::feed::acquire_record_surface(&pool, &skipped);
    assert!(denied.is_none(), "an exhausted pool must refuse, not reuse");
    assert_eq!(
        skipped.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the refusal counts as a skipped record frame, the same counter the \
         budget skip and the shed handoff feed"
    );

    // And the loan returns: the encoder finishing is the same event as the
    // compositor being allowed to draw the next frame.
    in_flight.pop();
    assert_eq!(pool.free(), 1);
    assert!(
        nbe_engine::record::feed::acquire_record_surface(&pool, &skipped).is_some(),
        "a returned surface must become available again, or the pool drains to \
         zero and the take silently stops recording"
    );
    assert_eq!(skipped.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn the_pool_never_hands_out_a_surface_that_is_still_in_flight() {
    let Some(device) = gpu_or_skip() else {
        eprintln!("SKIP: no wgpu adapter on this machine; the zero-copy chain needs one");
        return;
    };
    let pool = nbe_decode::zerocopy::SurfacePool::new(&device, 1920, 1080, pool_size())
        .expect("pool builds");
    let ids = pool.surface_ids();
    let distinct: std::collections::BTreeSet<u32> = ids.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "each pool member must be its OWN allocation; duplicates mean one \
         surface wearing several hats, which is the corruption the pool exists \
         to prevent: {ids:?}"
    );

    // Hand them all out and check no id repeats. A pool that "draws anyway" —
    // handing back a surface the encoder still holds — shows up here as the
    // same IOSurface id twice, which is a torn frame waiting to happen.
    let mut held = Vec::new();
    let mut handed: Vec<u32> = Vec::new();
    while let Some(s) = pool.acquire() {
        handed.push(s.surface_id());
        held.push(s);
        assert!(
            held.len() <= pool_size(),
            "acquire kept yielding past the pool's size: it is reusing surfaces"
        );
    }
    let handed_distinct: std::collections::BTreeSet<u32> = handed.iter().copied().collect();
    assert_eq!(
        handed_distinct.len(),
        handed.len(),
        "a surface was handed out twice while still in flight: {handed:?}"
    );
    assert_eq!(handed.len(), pool_size());
}

#[test]
fn the_encoder_refuses_a_surface_of_the_wrong_shape_rather_than_reinterpreting_it() {
    let Some(device) = gpu_or_skip() else {
        eprintln!("SKIP: no wgpu adapter on this machine; the zero-copy chain needs one");
        return;
    };
    if !nbe_engine::encode::is_available() {
        eprintln!("SKIP: no hardware H.264 encoder on this machine; the encode seam needs one");
        return;
    }
    let pool = nbe_decode::zerocopy::SurfacePool::new(&device, 1280, 720, 1).expect("pool builds");
    let surface = pool.acquire().expect("a fresh pool has a free surface");

    // A session at a DIFFERENT geometry. Without the check this buffer would be
    // interpreted rather than rejected, which is how a zero-copy path produces
    // a plausible-looking corrupt file instead of an error.
    let mut session = nbe_engine::encode::EncodeSession::open(1920, 1080, 30, 8_000_000)
        .expect("a hardware session at the reference geometry");
    let err = session
        .encode_pixel_buffer(surface.pixel_buffer())
        .expect_err("a 1280x720 buffer must not be encoded as 1920x1080");
    let msg = err.to_string();
    assert!(
        msg.contains("1280x720") && msg.contains("1920x1080"),
        "the refusal must name both shapes, got: {msg}"
    );

    // And the matching shape is accepted by the same call, so the refusal above
    // is about the mismatch and not about the path being unusable.
    let ok_pool =
        nbe_decode::zerocopy::SurfacePool::new(&device, 1920, 1080, 1).expect("pool builds");
    let ok_surface = ok_pool.acquire().expect("free");
    session
        .encode_pixel_buffer(ok_surface.pixel_buffer())
        .expect("the encoder takes the compositor's own allocation");
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
