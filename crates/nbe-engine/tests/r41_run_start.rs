//! R41 (filed 2026-11-01): `show.start` started the clock and left the render
//! loop asleep on its stopped grid, so a run's first tick came up to a budget
//! after frame 0 began. After a reload the last stopped tick ran about a
//! second, the start landed just behind it, and the loop slept a whole budget
//! past the epoch: the wake read frame 1. Measured at 6cb3738, 100 runs each,
//! on this machine's release build: frame 0 never drawn on 63 starts after a
//! reload and 2 settled ones; a frame dropped in the first second of 66 and 5.
//!
//! A start now ends the stopped sleep. The guards apply a real directive
//! beside the real `run_loop` on one thread, as in `main`, and count a sibling
//! task's polls from the directive's return to the run's first tick. A loop
//! the start woke draws on its next poll; a loop still asleep lets the count
//! run past the bound, however long the machine stalls. No timing is read, and
//! the frame the first tick reads is not pinned: a stall past the next boundary
//! may read a later one, and the identity is that the frames before it are
//! counted (R39).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::RenderLoop;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_engine::tick::run_loop;
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};

const RATE: u32 = 30;

/// Far more polls than a woken loop needs to reach its tick; far fewer than a
/// sibling makes while the loop sleeps a budget.
const POLLS: u32 = 100;

fn budget() -> Duration {
    Duration::from_secs_f64(1.0 / RATE as f64)
}

/// One thread, as in `main`.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a test runtime")
}

/// The engine state and its render loop, or a loud skip without an adapter.
async fn rig_or_skip() -> Option<(Arc<EngineState>, RenderLoop)> {
    let state = Arc::new(EngineState::new(RATE));
    match RenderLoop::new(state.clone()).await {
        Ok(render) => Some((state, render)),
        Err(e) => {
            eprintln!("SKIP: no GPU adapter for the render loop ({e})");
            None
        }
    }
}

fn show_start() -> DirectiveFrame {
    directive("show.start", 1, serde_json::json!({}))
}

/// `show.resync`, the second way a clock starts: a snapshot naming a running
/// show, as a reconnect replays it.
fn resync_running(state_version: u64) -> DirectiveFrame {
    directive(
        "show.resync",
        state_version,
        serde_json::json!({
            "showState": "RUNNING",
            "viewItem": null,
            "previewItem": null,
            "itemStates": {},
            "sceneStates": {},
            "visibleOverlays": [],
            "automationHold": false,
            "stateVersion": state_version
        }),
    )
}

fn directive(command: &str, state_version: u64, payload: serde_json::Value) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: state_version,
        state_version,
        command: command.into(),
        target: serde_json::json!({}),
        payload,
    }
}

/// `start` applied beside the loop once it has drawn a stopped tick of at
/// least `stopped_tick` and gone to sleep: where a directive queued behind that
/// tick applies. Reads the polls from the directive's return to the run's
/// first tick, and that tick's frame against the frames counted before it.
async fn start_beside_the_stopped_loop(start: DirectiveFrame, stopped_tick: Duration) {
    let Some((state, mut render)) = rig_or_skip().await else {
        return;
    };
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    let asleep = Rc::new(Cell::new(false));
    let first = Rc::new(Cell::new(None));
    let local = tokio::task::LocalSet::new();
    let polls = local
        .run_until(async {
            let sibling = {
                let (asleep, first) = (asleep.clone(), first.clone());
                tokio::task::spawn_local(async move {
                    while !asleep.get() {
                        tokio::task::yield_now().await;
                    }
                    handler.apply(&start).await.unwrap();
                    let mut polls = 0;
                    while first.get().is_none() && polls <= POLLS {
                        polls += 1;
                        tokio::task::yield_now().await;
                    }
                    polls
                })
            };
            run_loop(&mut render, &state, RATE, |report| {
                if report.master_frame.is_none() {
                    std::thread::sleep(stopped_tick);
                    asleep.set(true);
                }
                first.set(report.master_frame);
                first.get().is_none()
            })
            .await;
            sibling.await.expect("the sibling must not panic")
        })
        .await;
    assert!(
        polls <= POLLS,
        "the start left the loop asleep: {polls} polls and no running tick"
    );
    assert_eq!(
        state.skipped_master_frames.load(Ordering::SeqCst),
        first.get().expect("the run's first tick was reported"),
        "every frame before the run's first tick must be counted, and no other"
    );
}

#[test]
fn a_show_start_ends_the_stopped_sleep() {
    runtime().block_on(start_beside_the_stopped_loop(show_start(), Duration::ZERO));
}

/// The shape the rate measured: a stopped tick past its budget, as a reload's
/// ring upload is, and the start queued right behind it. The loop's grid has
/// resynchronized to a full budget ahead, past the clock's first boundary.
#[test]
fn a_show_start_behind_a_long_stopped_tick_ends_the_sleep_that_follows_it() {
    runtime().block_on(start_beside_the_stopped_loop(show_start(), budget() * 3));
}

#[test]
fn a_resync_that_starts_the_clock_ends_the_stopped_sleep() {
    runtime().block_on(start_beside_the_stopped_loop(
        resync_running(1),
        Duration::ZERO,
    ));
}

/// A resync of a show already running stores a wake nothing is waiting for.
/// It must stay out of the running sleep, which ends at the clock's boundary
/// and no earlier: a wake inside the frame would draw that frame twice.
#[test]
fn a_resync_of_a_running_show_leaves_the_running_sleep_alone() {
    runtime().block_on(async {
        let Some((state, mut render)) = rig_or_skip().await else {
            return;
        };
        let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
        handler.apply(&show_start()).await.unwrap();
        let frames = Rc::new(RefCell::new(Vec::new()));
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let sibling = {
                    let frames = frames.clone();
                    tokio::task::spawn_local(async move {
                        while frames.borrow().len() < 2 {
                            tokio::task::yield_now().await;
                        }
                        handler.apply(&resync_running(2)).await.unwrap();
                    })
                };
                run_loop(&mut render, &state, RATE, |report| {
                    frames.borrow_mut().extend(report.master_frame);
                    frames.borrow().len() < 6
                })
                .await;
                sibling.await.expect("the sibling must not panic");
            })
            .await;
        let frames = frames.borrow();
        assert!(
            frames.windows(2).all(|w| w[0] < w[1]),
            "a resync woke the running loop inside a frame: {frames:?}"
        );
    });
}
