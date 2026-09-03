//! nbe-engine: wgpu compositor, audio graph, and master clock (SPEC 6, 7, 10).
//!
//! Prompt 03 scope: render-node command bridge — the WebSocket client to the
//! control plane, the master clock, the directive handler, telemetry out, and
//! the watchdog skeleton. No GPU work until Prompt 04.

pub mod channel;
pub mod clock;
pub mod directive;
pub mod gpu;
pub mod render;
pub mod state;
pub mod telemetry;
pub mod watchdog;
