//! Prompt 11 WU8 — the watchdog's recovery half (SPEC §10.3, v0.4.7).
//!
//! §10.3 asked for a fault counter the engine did not keep, and the slate the
//! watchdog raised never came down (WU6's finding). Here: trips and clears are
//! counted and ride the §10.1 tick; a tripped slate clears after
//! `WATCHDOG_CLEAR_AFTER_ON_TIME` consecutive on-time View frames, with
//! hysteresis; and the recovery releases only the watchdog's own slate.
//!
//! Through the real render loop: lateness is injected as View work and
//! measured by the loop itself. Every test renders an on-time warm-up frame
//! first — a cold adapter's first render cost 143 ms on the CI runner (run
//! 36356479347) — and the budgets leave that much headroom and more. GPU
//! required; no adapter fails loudly, as in prompt04.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, FallbackSource, OutgoingQueue};
use nbe_engine::watchdog::WATCHDOG_CLEAR_AFTER_ON_TIME as K;
use nbe_protocol::{DirectiveFrame, DirectiveKind, EngineFrame, PROTOCOL_VERSION};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// A generous budget: with no injected work, a frame is on time.
const ON_TIME: Duration = Duration::from_secs(5);
/// Trip frames: 700 ms of work against 200 ms — late by at least 500 ms,
/// `ceil(late / budget)` ≥ 3 > 2, so ONE frame trips, whatever the render
/// itself costs.
const TRIP_BUDGET: Duration = Duration::from_millis(200);
const TRIP_WORK: Duration = Duration::from_millis(700);
/// A late frame that is not a trip on its own: 210 ms against 200.
const JUST_LATE: Duration = Duration::from_millis(210);

fn directive(command: &str, sv: u64, payload: serde_json::Value) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target: serde_json::json!({}),
        payload,
    }
}

fn make_package(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        64,
        32,
        image::Rgba([10, 20, 30, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": {"width":1920,"height":1080,"frameRate":30,"colorSpace":"rec709"},
                "audio": {"sampleRate":48000,"loudnessTargetLufs":-16,"truePeakDbtp":-1.5},
                "fallbackAssetId": "slate"
            },
            "qualityProfile": "potato",
            "assets": [{ "id": "slate", "kind": "image", "source": "media/slate.png" }],
            "scenes": [
                { "id": "SCN", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ff0000" } }
                ]}
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

/// A loaded engine with A1 on the View, one on-time warm-up frame rendered.
async fn engine() -> (
    tempfile::TempDir,
    Arc<EngineState>,
    DirectiveHandler,
    RenderLoop,
    u64,
) {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path());
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    let mut render = match RenderLoop::new(state.clone()).await {
        Ok(r) => r,
        Err(e) => panic!("wgpu adapter unavailable; tests must FAIL loudly, not skip: {e}"),
    };
    *state.view_item.lock().unwrap() = Some("A1".into());
    assert!(render.render_frame(0, Some(ON_TIME)).view_late_by.is_none());
    (dir, state, handler, render, 1)
}

fn trip(render: &mut RenderLoop, frame: &mut u64) {
    render.injected_view_delay = Some(TRIP_WORK);
    let late = render.render_frame(*frame, Some(TRIP_BUDGET)).view_late_by;
    *frame += 1;
    assert!(
        late.is_some_and(|l| l > 2 * TRIP_BUDGET),
        "a trip frame is late by more than two budgets: {late:?}"
    );
}

fn on_time(render: &mut RenderLoop, frame: &mut u64, n: u64) {
    render.injected_view_delay = None;
    for _ in 0..n {
        assert!(render
            .render_frame(*frame, Some(ON_TIME))
            .view_late_by
            .is_none());
        *frame += 1;
    }
}

fn just_late(render: &mut RenderLoop, frame: &mut u64) {
    render.injected_view_delay = Some(JUST_LATE);
    let late = render.render_frame(*frame, Some(TRIP_BUDGET)).view_late_by;
    *frame += 1;
    assert!(
        late.is_some_and(|l| l <= TRIP_BUDGET),
        "late by at most one budget: {late:?}"
    );
}

fn counts(state: &EngineState) -> (u64, u64) {
    (
        state.watchdog_trips_total.load(Ordering::SeqCst),
        state.watchdog_clears_total.load(Ordering::SeqCst),
    )
}

#[tokio::test]
async fn a_tripped_watchdog_slate_comes_down_after_k_on_time_frames() {
    let (_dir, state, _h, mut render, mut f) = engine().await;
    trip(&mut render, &mut f);
    assert!(state.fallback_active(), "the trip puts the slate up");
    assert!(state.fallback_held_by(FallbackSource::Watchdog));

    on_time(&mut render, &mut f, K - 1);
    assert!(
        state.fallback_active(),
        "{} on-time frames are one short of K = {K}: the slate stays",
        K - 1
    );
    on_time(&mut render, &mut f, 1);
    assert!(
        !state.fallback_active(),
        "K = {K} consecutive on-time frames take the watchdog's slate down"
    );
}

#[tokio::test]
async fn trips_and_clears_are_counted_once_each_and_ride_the_tick() {
    let (_dir, state, _h, mut render, mut f) = engine().await;
    assert_eq!(counts(&state), (0, 0));

    // A trip, then more late frames while the slate is up: ONE fault.
    trip(&mut render, &mut f);
    trip(&mut render, &mut f);
    just_late(&mut render, &mut f);
    assert_eq!(
        counts(&state),
        (1, 0),
        "one trip per episode, not per frame"
    );
    on_time(&mut render, &mut f, K);
    assert_eq!(counts(&state), (1, 1), "the recovery is counted");

    // A second episode.
    trip(&mut render, &mut f);
    assert_eq!(counts(&state), (2, 1));
    on_time(&mut render, &mut f, K);
    assert_eq!(counts(&state), (2, 2));

    // On the §10.1 tick, beside degradationRung (§10.5's reporting shape).
    match nbe_engine::telemetry::build_tick(&state) {
        EngineFrame::EngineTelemetry { fields, .. } => {
            assert_eq!(
                (fields.watchdog_trips_total, fields.watchdog_clears_total),
                (2, 2),
                "the tick carries the counters"
            );
            assert!(!fields.fallback_active);
        }
        other => panic!("expected engineTelemetry, got {other:?}"),
    }
}

#[tokio::test]
async fn one_on_time_frame_does_not_clear_and_a_late_frame_restarts_the_run() {
    let (_dir, state, _h, mut render, mut f) = engine().await;
    trip(&mut render, &mut f);

    // Hysteresis: one on-time frame in a bad patch clears nothing.
    on_time(&mut render, &mut f, 1);
    assert!(
        state.fallback_active(),
        "one on-time frame must NOT clear a tripped slate (K = {K})"
    );
    // The run must be unbroken: K − 1 on time, one late, K − 1 on time is
    // 2K − 2 on-time frames and still no clear.
    on_time(&mut render, &mut f, K - 2);
    just_late(&mut render, &mut f);
    on_time(&mut render, &mut f, K - 1);
    assert!(
        state.fallback_active(),
        "a late frame restarts the on-time run"
    );
    on_time(&mut render, &mut f, 1);
    assert!(
        !state.fallback_active(),
        "K unbroken on-time frames clear it"
    );
    assert_eq!(counts(&state), (1, 1));
}

#[tokio::test]
async fn the_recovery_never_releases_an_operators_slate() {
    let (_dir, state, handler, mut render, mut f) = engine().await;
    // The operator's view.fallback, and a watchdog trip on top of it.
    handler
        .apply(&directive("view.fallback", 2, serde_json::json!({})))
        .await
        .unwrap();
    *state.view_item.lock().unwrap() = Some("A1".into());
    trip(&mut render, &mut f);
    assert!(state.fallback_held_by(FallbackSource::Held));
    assert!(state.fallback_held_by(FallbackSource::Watchdog));

    on_time(&mut render, &mut f, K);
    assert!(
        !state.fallback_held_by(FallbackSource::Watchdog),
        "the watchdog let go of its slate"
    );
    assert!(
        state.fallback_active(),
        "the operator's slate is still on air: the recovery releases only its own"
    );
    assert_eq!(counts(&state), (1, 1));
}
