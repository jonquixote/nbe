//! Finding R11 — how long does the stream thread's exit really take? (the
//! user's word of 2026-10-10). The measurement, not a test.
//!
//! `#[ignore]`d: a number measured under load is worse than no number
//! (`docs/soak-protocol.md` §2). It runs ONE stream teardown per process, so
//! each run opens VideoToolbox cold, as each `cargo test` process of the lib
//! suite does. A script drives it a hundred times per configuration, on AC
//! under `caffeinate`, waiting for the load before each run:
//!
//! ```text
//! R11_CONFIG=immediate cargo test -p nbe-engine --test r11_exit_path -- --ignored --nocapture
//! R11_CONFIG=running   cargo test -p nbe-engine --test r11_exit_path -- --ignored --nocapture
//! ```
//!
//! The configuration is the stream tests' own: `StreamSession::open` on
//! `rtmp://127.0.0.1:9/live/k` (nothing listens, so the publisher redials in
//! the background), the selection `select_stream(true)`, 640×360 at 30 fps,
//! and production's exit path (no exit delay: that seam is `cfg(test)`).
//! - `immediate`: stop straight after open, as the lib suite's stream tests
//!   do. The thread may still be opening its encoders when `Stop` arrives.
//! - `running`: stop once the encoders are open and the thread has run
//!   100 ms, as a show's `stream.stop` does.
//!
//! Each run prints one `R11-TEARDOWN` line: the configuration, the load, the
//! result, the wall time of `stop_and_close` measured from outside, and the
//! session's teardown marks (`StreamStats::teardown`, µs since open).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::record::stream::{StreamParams, StreamSession, TeardownMark};

const LOAD_CEILING: f64 = 3.0;

fn load_1m() -> f64 {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .expect("sysctl must run");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_matches(|c| c == '{' || c == '}' || c == ' ')
        .split_whitespace()
        .next()
        .expect("a load average")
        .parse()
        .expect("a number")
}

#[tokio::test]
#[ignore = "measurement: R11's exit path, one teardown per process, driven by a script"]
async fn r11_one_teardown() {
    let config = std::env::var("R11_CONFIG").unwrap_or_else(|_| "immediate".into());
    let load = load_1m();
    assert!(
        load < LOAD_CEILING,
        "VOID: load {load:.2} at or over the {LOAD_CEILING} ceiling"
    );
    let sel = nbe_engine::record::tap_path::select_stream(true).expect("a stream selection");
    let mut s = StreamSession::open(
        "rtmp://127.0.0.1:9/live/k",
        sel,
        StreamParams::new(640, 360, 30),
        Arc::new(AtomicU64::new(0)),
    );
    let stats = s.stats();
    match config.as_str() {
        "immediate" => {}
        "running" => {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !stats.encoder_ready.load(Ordering::SeqCst) && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            assert!(
                stats.encoder_ready.load(Ordering::SeqCst),
                "the encoder never opened"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        other => panic!("R11_CONFIG must be immediate or running, got {other}"),
    }
    let t0 = Instant::now();
    let result = s.stop_and_close().await;
    let external_us = t0.elapsed().as_micros() as u64;
    let marks: serde_json::Map<String, serde_json::Value> = TeardownMark::ALL
        .iter()
        .map(|&m| {
            (
                m.as_str().to_string(),
                stats
                    .teardown
                    .at_us(m)
                    .map_or(serde_json::Value::Null, |v| v.into()),
            )
        })
        .collect();
    println!(
        "R11-TEARDOWN {}",
        serde_json::json!({
            "config": config,
            "load1m": load,
            "ok": result.is_ok(),
            "err": result.err().map(|e| e.to_string()),
            "externalUs": external_us,
            "encoderOpenUs": stats.encoder_open_us.load(Ordering::SeqCst),
            "marksUs": marks,
        })
    );
}
