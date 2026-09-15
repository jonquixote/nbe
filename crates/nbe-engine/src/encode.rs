//! Recording encoder surface (Prompt 09 WU2, SPEC §9.2).
//!
//! The session itself lives in `nbe-decode` — VideoToolbox encode is `unsafe`
//! FFI and this crate denies `unsafe_code` — and is re-exported here so
//! engine paths read the same (cf. `pub use nbe_decode as decode` in
//! `lib.rs`). This module contains no `unsafe` and adds no FFI dependency.
//!
//! The record pipeline (WU-pipe) owns the ONE live encoder on the record
//! thread at View geometry; `record.start` only reserves naming and flips
//! state, and encoder absence degrades at feed/finish time (skipped frames,
//! loud finish) rather than refusing the start.

pub use nbe_decode::encode::{is_available, EncodeError, EncodeSession, EncodedUnit};
