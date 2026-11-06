//! R36 (filed 2026-10-28, cross-referenced to R33): `recordSpaceMib` was
//! measured on the telemetry tick, a tokio worker, by forking `df`. While a
//! take recorded the fork blocked the worker for one to two seconds, and the
//! render loop's sleep — a tokio timer — ended that late: 28–63 master frames
//! never drawn, about once per ten seconds. The measurement now runs on its
//! own thread (`telemetry::RecordSpace`); these guards give it a volume that
//! answers only when the test lets it, so a measurement outlasts whatever the
//! test does beside it on any machine, and no duration is read (R39).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use nbe_engine::render::RenderLoop;
use nbe_engine::state::EngineState;
use nbe_engine::telemetry::{build_tick_for_dir, RecordSpace};
use nbe_protocol::EngineFrame;

const MEASURED: f64 = 4242.0;
/// Past this a wait is a hang, and its test fails: it turns a hang into a red
/// and measures nothing. A guard that holds never waits this long.
const HANG: Duration = Duration::from_secs(30);

/// A volume that holds its measurement until the test lets it go.
struct Volume {
    open: Mutex<bool>,
    opened: Condvar,
    measuring: AtomicBool,
    gave_up: AtomicBool,
}

impl Volume {
    const fn new() -> Self {
        Self {
            open: Mutex::new(false),
            opened: Condvar::new(),
            measuring: AtomicBool::new(false),
            gave_up: AtomicBool::new(false),
        }
    }

    fn measure(&self) -> f64 {
        self.measuring.store(true, Ordering::SeqCst);
        let open = self.open.lock().unwrap();
        let (_open, wait) = self
            .opened
            .wait_timeout_while(open, HANG, |open| !*open)
            .unwrap();
        self.gave_up.store(wait.timed_out(), Ordering::SeqCst);
        MEASURED
    }

    fn let_go(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

// `RecordSpace`'s probe is a plain `fn`, so each test has its own volume.
static TICK_VOLUME: Volume = Volume::new();
static LOOP_VOLUME: Volume = Volume::new();

fn tick_volume(_: &Path) -> f64 {
    TICK_VOLUME.measure()
}

fn loop_volume(_: &Path) -> f64 {
    LOOP_VOLUME.measure()
}

fn space(frame: EngineFrame) -> f64 {
    match frame {
        EngineFrame::EngineTelemetry { fields, .. } => fields.record_space_mib,
        other => panic!("expected engineTelemetry, got {other:?}"),
    }
}

fn state_on_a_volume(probe: fn(&Path) -> f64) -> EngineState {
    let mut state = EngineState::new(30);
    state.record_space = RecordSpace::with_probe(probe);
    state
}

/// One telemetry tick, on its own thread: a tick that waits on the volume is
/// then a red and not a hang.
fn tick(state: &Arc<EngineState>, dir: &Path) -> f64 {
    let (state, dir) = (state.clone(), dir.to_path_buf());
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || done.send(space(build_tick_for_dir(&state, Some(&dir)))));
    finished
        .recv_timeout(HANG)
        .expect("the tick waited on the volume")
}

#[test]
fn the_telemetry_tick_never_waits_on_a_slow_volume() {
    let state = Arc::new(state_on_a_volume(tick_volume));
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(tick(&state, dir.path()), 0.0, "nothing is measured yet");
    // The measurement is inside the volume now and stays there: a later tick
    // beside it answers all the same.
    let end = Instant::now() + HANG;
    while !TICK_VOLUME.measuring.load(Ordering::SeqCst) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        TICK_VOLUME.measuring.load(Ordering::SeqCst),
        "the tick never asked for a measurement"
    );
    assert_eq!(
        tick(&state, dir.path()),
        0.0,
        "still measuring, nothing to carry yet"
    );
    TICK_VOLUME.let_go();
    // The measurement lands on its own thread, and the tick then carries it.
    let end = Instant::now() + HANG;
    let mut v = 0.0;
    while v != MEASURED && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
        v = tick(&state, dir.path());
    }
    assert_eq!(v, MEASURED, "the measurement never reached the tick");
}

// One worker, as the driver of every timer: a measurement that blocks a worker
// then stops the render loop's sleep from ending, on any machine, which is
// R36's path with no luck in it. With several, another worker can take the
// driver up and the loop may draw on.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_slow_volume_never_stalls_the_render_loop() {
    let state = Arc::new(state_on_a_volume(loop_volume));
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
    // The volume holds its measurement until the loop has drawn these beside
    // it. A loop stalled behind the measurement never gets there.
    const DRAWN: u32 = 30;
    let mut during = 0;
    let mut worst = Duration::ZERO;
    nbe_engine::tick::run_loop(&mut render, &state, 30, |r| {
        worst = worst.max(r.woke_late);
        during += u32::from(LOOP_VOLUME.measuring.load(Ordering::SeqCst));
        during < DRAWN && !LOOP_VOLUME.gave_up.load(Ordering::SeqCst)
    })
    .await;
    LOOP_VOLUME.let_go();
    pump.abort();
    eprintln!("r36: {during} ticks beside a held measurement; worst wake {worst:?}");
    assert_eq!(
        during, DRAWN,
        "the render loop stalled behind the volume's measurement"
    );
}
