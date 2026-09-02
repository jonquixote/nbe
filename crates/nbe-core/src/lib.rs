//! nbe-core: shared types, manifest model, and rundown state machine.
//! Normative spec: SPEC v0.3.1 (`docs/spec.v0.3.md`).

pub mod manifest;
pub mod preflight;
pub mod validate;

pub use manifest::*;
pub use preflight::*;
pub use validate::{validate_manifest, ValidationError};
