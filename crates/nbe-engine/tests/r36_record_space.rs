//! R36 (filed 2026-10-28, cross-referenced to R33): `recordSpaceMib` was
//! measured on the telemetry tick, a tokio worker, by forking `df`. While a
//! take recorded the fork blocked the worker for one to two seconds, and the
//! render loop's sleep — a tokio timer — ended that late: 28–63 master frames
//! never drawn, about once per ten seconds. The measurement now runs on its
//! own thread (`telemetry::RecordSpace`); these guards give it a volume that
//! takes 1.5 s to measure.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::render::RenderLoop;
use nbe_engine::state::EngineState;
use nbe_engine::telemetry::{build_tick_for_dir, RecordSpace};
use nbe_protocol::EngineFrame;

const SLOW: Duration = Duration::from_millis(1500);
const MEASURED: f64 = 4242.0;

fn slow_volume(_: &Path) -> f64 {
    std::thread::sleep(SLOW);
    MEASURED
}

fn space(frame: EngineFrame) -> f64 {
    match frame {
        EngineFrame::EngineTelemetry { fields, .. } => fields.record_space_mib,
        other => panic!("expected engineTelemetry, got {other:?}"),
    }
}

fn state_on_a_slow_volume() -> EngineState {
    let mut state = EngineState::new(30);
    state.record_space = RecordSpace::with_probe(slow_volume);
    state
}

#[test]
fn the_telemetry_tick_never_waits_on_a_slow_volume() {
    let state = state_on_a_slow_volume();
    let dir = tempfile::tempdir().expect("tempdir");
    let started = Instant::now();
    let first = space(build_tick_for_dir(&state, Some(dir.path())));
    let took = started.elapsed();
    assert!(
        took < Duration::from_millis(200),
        "the tick waited {took:?} on the volume"
    );
    assert_eq!(first, 0.0, "nothing is measured yet");
    // The measurement lands on its own thread, and the tick then carries it.
    let end = Instant::now() + SLOW * 3;
    let mut v = first;
    while v != MEASURED && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        v = space(build_tick_for_dir(&state, Some(dir.path())));
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "a later tick waited on the volume"
        );
    }
    assert_eq!(v, MEASURED, "the measurement never reached the tick");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_volume_never_stalls_the_render_loop() {
    let state = Arc::new(state_on_a_slow_volume());
    let render = match RenderLoop::new(state.clone()).await {
        Ok(render) => render,
        Err(e) => {
            eprintln!("SKIP: no GPU adapter for the render loop ({e})");
            return;
        }
    };
    let mut render = render;
    let dir = tempfile::tempdir().expect("tempdir");
    // The telemetry pump's shape: a worker task building ticks against the
    // record target.
    let pump = {
        let state = state.clone();
        let dir = dir.path().to_path_buf();
        tokio::spawn(async move {
            loop {
                let _ = build_tick_for_dir(&state, Some(&dir));
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    };
    state.clock.lock().unwrap().start();
    let mut late = Vec::new();
    nbe_engine::tick::run_loop(&mut render, &state, 30, |r| {
        late.push(r.woke_late);
        late.len() < 120
    })
    .await;
    pump.abort();
    let worst = late.iter().max().copied().unwrap_or_default();
    eprintln!("r36: 120 ticks beside a 1.5 s volume; worst wake {worst:?}");
    assert!(
        worst < Duration::from_millis(500),
        "the render loop woke {worst:?} late while the volume was measured"
    );
}
