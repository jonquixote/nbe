//! nbe-engine: wgpu compositor, audio graph, and master clock (SPEC 6, 7, 10).
//!
//! Prompt 03 scope: render-node command bridge — the WebSocket client to the
//! control plane, the master clock, the directive handler, telemetry out, and
//! the watchdog skeleton. No GPU work until Prompt 04.

pub mod audio;
pub mod audio_control;
pub mod audio_driver;
pub mod channel;
pub mod clock;
/// Hardware decode lives in its own crate so `nbe-preflight` can use it
/// without pulling in wgpu. Re-exported here so engine paths read the same.
pub use nbe_decode as decode;
pub mod directive;
pub mod gpu;
/// SPEC §12.5's budget decision lives in `nbe-core` so preflight and the
/// engine cannot disagree about it. Re-exported here for callers.
pub use nbe_core::loop_cache;
pub mod render;
pub mod scene;
pub mod state;
pub mod telemetry;
pub mod video;
pub mod watchdog;
