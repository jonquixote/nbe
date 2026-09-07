//! nbe-core: shared types, manifest model, and rundown state machine.
//! Normative spec: SPEC v0.4 (`docs/spec.v0.4.md`).

pub mod manifest;
pub mod preflight;
pub mod validate;

pub use manifest::*;
pub mod loop_cache;
pub use preflight::*;
pub use validate::{validate_manifest, ValidationError};
