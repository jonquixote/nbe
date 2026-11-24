//! SPEC §11.4, §10.1 (v0.4.15 row 1): the tick carries the engine's own
//! master clock state. `masterClockFrame` reads 0 while the clock is STOPPED,
//! so on its own a stopped clock was indistinguishable from a running clock's
//! first frame, and the control plane reported its own `showState` as the
//! render node's clock (the v0.5 outline's §1 item 9).

use nbe_engine::state::EngineState;

fn tick(state: &EngineState) -> serde_json::Value {
    serde_json::to_value(nbe_engine::telemetry::build_tick(state)).unwrap()
}

#[test]
fn the_tick_carries_the_engines_own_clock_state() {
    let state = EngineState::new(30);
    let before = tick(&state);
    assert_eq!(
        before["masterClockState"], "STOPPED",
        "a clock not yet started is STOPPED"
    );
    assert_eq!(
        before["masterClockFrame"], 0,
        "and its frame reads 0, which alone could be a running clock's first frame"
    );

    state.start_clock();
    assert_eq!(
        tick(&state)["masterClockState"],
        "RUNNING",
        "a started clock is RUNNING"
    );
}
