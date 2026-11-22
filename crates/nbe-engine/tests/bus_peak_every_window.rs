//! Every closed meter window reaches a telemetry tick (SPEC §10.1's note:
//! "a transient shorter than a telemetry interval still reaches the
//! operator").
//!
//! The audio driver closes a meter window every `blocks_per_window` blocks of
//! audio and published it by overwriting `bus_peaks`; the tick read whatever
//! was there at its own wall-clock time. When the audio thread runs behind
//! and catches up in a burst (CI's null sink, several times a second), two
//! windows can close between ticks, and the first was never read. A 0.4 s
//! soundboard stab sits in one window and every window after it is silent,
//! so one lost window is dress step 6's failure: "waited 10000 ms for
//! busPeakDbfs.sfx to rise", with the trigger applied. (That this is what
//! failed step 6 on CI is the hypothesis this test makes checkable; the loss
//! itself is shown here.)

use std::sync::Arc;

use nbe_engine::audio::{db_to_linear, BusId, Source};
use nbe_engine::audio_driver::{AudioDriver, NullSink};
use nbe_engine::state::EngineState;
/// `busPeakDbfs` as the real tick builds it (the channel pump's path).
fn bus_peaks_for_tick(state: &EngineState) -> std::collections::BTreeMap<String, f64> {
    match nbe_engine::telemetry::build_tick_for_dir(state, None) {
        nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } => fields.bus_peak_dbfs,
        other => panic!("a tick is engine telemetry, got {other:?}"),
    }
}

const HOUSE_RATE: u32 = 30;
const BLOCK: usize = 48_000 / HOUSE_RATE as usize;

fn driver() -> (Arc<EngineState>, AudioDriver) {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let mut driver = AudioDriver::new(state.clone(), Box::new(NullSink::new(BLOCK)), HOUSE_RATE);
    // Two blocks a window: short, but a real window boundary.
    driver.set_meter_window(driver.block_duration() * 2);
    (state, driver)
}

fn stab(driver: &mut AudioDriver) {
    driver.graph.set_source(
        BusId::Sfx,
        vec![Source::Tone {
            hz: 1000.0,
            amplitude: db_to_linear(-6.0),
        }],
    );
}

#[test]
fn a_stab_window_replaced_before_the_tick_still_reaches_the_tick() {
    let (state, mut driver) = driver();
    // Window 1: the stab. Window 2: silence. Both close before any tick: the
    // audio thread caught up in one burst.
    stab(&mut driver);
    driver.cycle(0);
    driver.cycle(1);
    driver.graph.set_source(BusId::Sfx, Vec::new());
    driver.cycle(2);
    driver.cycle(3);
    let latest = state.bus_peaks.lock().unwrap().get("sfx").copied().unwrap();
    assert!(
        latest < -60.0,
        "precondition: the latest window is silent, {latest:.1}"
    );

    let tick = bus_peaks_for_tick(&state);
    let sfx = tick.get("sfx").copied().expect("sfx is metered");
    assert!(
        (sfx - -6.0).abs() < 1.5,
        "the tick must carry the stab's window, not only the latest: sfx {sfx:.1} dBFS"
    );

    // The next tick, with no window closed since, holds the latest (silent)
    // window: the stab is reported once, and the meter falls again.
    let next = bus_peaks_for_tick(&state);
    let sfx = next.get("sfx").copied().unwrap();
    assert!(
        sfx < -60.0,
        "a meter falls once its window has been read: {sfx:.1}"
    );
}

#[test]
fn a_tick_with_no_new_window_holds_the_latest_rather_than_blanking() {
    let (state, mut driver) = driver();
    stab(&mut driver);
    driver.cycle(0);
    driver.cycle(1);
    let first = bus_peaks_for_tick(&state);
    // The audio thread is behind: no window closes before the next tick.
    let second = bus_peaks_for_tick(&state);
    assert_eq!(
        first, second,
        "a tick with nothing new repeats the latest window"
    );
    assert!(second.get("sfx").is_some_and(|p| (p - -6.0).abs() < 1.5));
}
