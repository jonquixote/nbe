//! Recording encoder surface (Prompt 09 WU2, SPEC §9.2).
//!
//! The session itself lives in `nbe-decode` — VideoToolbox encode is `unsafe`
//! FFI and this crate denies `unsafe_code` — and is re-exported here so
//! engine paths read the same (cf. `pub use nbe_decode as decode` in
//! `lib.rs`). This module contains no `unsafe` and adds no FFI dependency.
//!
//! The record pipeline (WU-pipe) owns the ONE live encoder on the record
//! thread at View geometry, opened at the first take.
//!
//! **`record.start` refuses when no hardware encoder answers.** SPEC §9.2 is a
//! MUST — "If no hardware encoder is available, output start MUST fail with
//! `E_NO_HARDWARE_ENCODER`" — and §16.14 lists "encoder available" among the
//! command's preconditions. `Directive::on_record_start` probes
//! [`is_available`] through `record::session::encoder_available` (seam-aware,
//! no stream opened) and returns `E_NO_HARDWARE_ENCODER`; start also
//! admission-probes free space and checks the resolution ceiling, so it does
//! more than reserve naming. Pinned by
//! `record_start_with_failed_encoder_probe_is_refused`.
//!
//! This paragraph previously said the opposite — that absence "degrades at
//! feed/finish time rather than refusing the start" — which described a spec
//! violation the code never committed. That was true of an intermediate design
//! only: `e34166d` added the refusal, `64d8f5c` retired the premise in the
//! tests, and `11fb757` restored it with the pipeline; the prose did not follow
//! the third step. Recorded rather than silently overwritten (§2c) so the next
//! reader does not re-derive it.
//!
//! Absence *after* a take is under way is a different question, and there the
//! old sentence still holds: the feed path sheds frames and the finish is loud
//! rather than pretending success.

pub use nbe_decode::encode::{is_available, EncodeError, EncodeSession, EncodedUnit};
