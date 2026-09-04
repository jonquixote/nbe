//! nbe-engine: wgpu compositor, audio graph, and master clock (SPEC 6, 7, 10).
//!
//! Prompt 03 scope: render-node command bridge — the WebSocket client to the
//! control plane, the master clock, the directive handler, telemetry out, and
//! the watchdog skeleton. No GPU work until Prompt 04.

pub mod channel;
pub mod clock;
/// Hardware decode lives in its own crate so `nbe-preflight` can use it
/// without pulling in wgpu. Re-exported here so engine paths read the same.
pub use nbe_decode as decode;
pub mod directive;
pub mod gpu;
pub mod loop_cache;
pub mod render;
pub mod scene;
pub mod state;
pub mod telemetry;
pub mod video;
pub mod watchdog;
