//! The wgpu-core lock probe (the vendored `lock/vanilla.rs`, NBE patch of
//! 2026-11-24): with `NBE_LOCK_TIMING=1`, an acquire that waited at least
//! `NBE_LOCK_TIMING_MS` logs the lock's name, the waiting thread and the last
//! holder before it. It is the instrument for the slow-GPU-poll question:
//! which thread held the lock while the render thread's non-blocking poll
//! waited 20-39 ms on CI (`UploadPool::reclaim`).
//!
//! Its own process (one test, one binary): the probe reads its environment
//! once, at the first wgpu-core lock in the process.

use std::sync::Mutex;

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.target() == "nbe_lock"
    }
    fn log(&self, record: &log::Record) {
        if record.target() == "nbe_lock" {
            LINES.lock().unwrap().push(record.args().to_string());
        }
    }
    fn flush(&self) {}
}

static CAPTURE: Capture = Capture;

#[test]
fn the_lock_probe_names_the_lock_the_waiter_and_the_last_holder() {
    // Before any wgpu-core lock in this process: the probe reads them once.
    std::env::set_var("NBE_LOCK_TIMING", "1");
    // A threshold of 0 reports every acquire, so the test does not depend on a
    // wait the machine happens to produce.
    std::env::set_var("NBE_LOCK_TIMING_MS", "0");
    log::set_logger(&CAPTURE).unwrap();
    log::set_max_level(log::LevelFilter::Warn);

    let gpu =
        pollster::block_on(nbe_engine::gpu::Gpu::init()).expect("the probe needs a wgpu adapter");
    let device = gpu.device.clone();
    let queue = gpu.queue.clone();
    // A named thread takes the device's locks first, so the main thread's
    // next acquire names it as the last holder.
    std::thread::Builder::new()
        .name("probe-holder".into())
        .spawn(move || {
            let enc = device.create_command_encoder(&Default::default());
            queue.submit([enc.finish()]);
            let _ = device.poll(wgpu::PollType::Poll);
        })
        .unwrap()
        .join()
        .unwrap();
    let _ = gpu.device.poll(wgpu::PollType::Poll);

    let lines = LINES.lock().unwrap().clone();
    assert!(
        !lines.is_empty(),
        "with NBE_LOCK_TIMING=1 and a 0 ms threshold, every wgpu-core acquire reports"
    );
    assert!(
        lines
            .iter()
            .all(|l| l.starts_with("NBE-LOCK ") && l.contains(" waited ")),
        "each report names the lock and the wait: {:?}",
        &lines[..lines.len().min(3)]
    );
    assert!(
        lines.iter().any(|l| l.contains("Device::snatchable_lock")),
        "the device's snatch lock is named, as the slow polls take it: {:?}",
        &lines[..lines.len().min(3)]
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("probe-holder") && l.contains("; last ")),
        "a report names the holder thread: {} lines, none name probe-holder as holder or waiter",
        lines.len()
    );
}
