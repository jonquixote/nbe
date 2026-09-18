//! AAC encode for recording: the safe integration layer (Prompt 09 WU34).
//!
//! The converter itself lives in `nbe_decode::aac` — every AudioToolbox call
//! is `unsafe` by construction and this crate denies `unsafe_code`, so the
//! FFI cannot live here (same split as `crate::encode` over
//! `nbe_decode::encode`). This module re-exports that safe API and documents
//! the one rule the writer enforces: AudioToolbox absent means the recording
//! fails loudly (`RecordError::Aac`), never a silent PCM substitution.

pub use nbe_decode::aac::{
    AacEncoder, AacError, AacFrame, FRAMES_PER_PACKET, INPUT_CHANNELS, INPUT_SAMPLE_RATE,
};

/// True when AudioToolbox answers a full probe encode (open + one packet +
/// cookie). The recording tests assert this loudly instead of probing around
/// a missing codec: no AAC means no recording, not a different recording.
pub fn is_available() -> bool {
    AacEncoder::is_available()
}
