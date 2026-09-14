//! Recording encoder surface (Prompt 09 WU2, SPEC §9.2).
//!
//! The session itself lives in `nbe-decode` — VideoToolbox encode is `unsafe`
//! FFI and this crate denies `unsafe_code` — and is re-exported here so
//! engine paths read the same (cf. `pub use nbe_decode as decode` in
//! `lib.rs`). This module contains no `unsafe` and adds no FFI dependency.
//!
//! WU2 scope: the session and its `is_available()` probe exist, but
//! `record.start` wiring still refuses with `E_NO_HARDWARE_ENCODER` (WU1
//! behavior); flipping that integration is a later work unit.

pub use nbe_decode::encode::{is_available, EncodeError, EncodeSession, EncodedUnit};
