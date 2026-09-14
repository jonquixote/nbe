//! Crash-safe recording file output (Prompt 09 WU34, SPEC §9.3).
//!
//! The record thread owns a [`RecordingWriter`]: it drains the [`AudioTap`]
//! ring (fed by the real-time audio thread, which never touches the
//! filesystem) and the video access units (WU2 `EncodedUnit`), and appends
//! fragmented-MP4 fragments. See `writer.rs` for the crash-safety shape and
//! `audio_tap.rs` for the tap contract.
//!
//! ```text
//! audio thread: master mix PCM ──push──▶ AudioTap (ring, no I/O)
//! record thread: tap.drain() + video units ──▶ RecordingWriter ──▶ file
//! ```
//!
//! In-scope for this work unit (reviewer scope note): the `EncodeSession`
//! SPS/PPS capture in `nbe-decode/src/encode.rs`
//! (`EncodeSession::parameter_sets` from the first keyframe's format
//! description) → [`RecordParams::sps`]/[`RecordParams::pps`] → the file's
//! `avcC` box in `writer.rs`. VideoToolbox emits no parameter sets in-band,
//! so the file has no other source for them.
//!
//! [`write_recording`] is the one-shot form (the whole take is in memory);
//! [`RecordingWriter`] is the streaming form (fragments flush as they
//! complete, so a kill retains them).

pub mod aac;
pub mod audio_tap;
pub mod markers;
pub mod writer;

pub use audio_tap::{AudioTap, DEFAULT_CAPACITY_SAMPLES};
pub use writer::{
    write_recording, RecordingWriter, AUDIO_TIMESCALE, AUDIO_TRACK_ID, VIDEO_TIMESCALE,
    VIDEO_TRACK_ID,
};

use std::path::PathBuf;
use thiserror::Error;

/// Recording failure. thiserror, with stable `E_` tokens for operator-facing
/// state; `anyhow` appears in no library path (binaries only, of which this
/// work unit has none).
#[derive(Debug, Error)]
pub enum RecordError {
    /// The target volume refused the write (permission, missing component,
    /// full disk — any `io::Error` on create/write/flush/seek).
    #[error("E_DISK: {0}")]
    Disk(String),
    /// AudioToolbox AAC refused (probe, open, or mid-stream). Fails loudly
    /// by design: there is no PCM-substitution path.
    #[error("E_AAC_UNAVAILABLE: {0}")]
    Aac(String),
    /// The inputs cannot become a recording (no units, no IDR, odd audio,
    /// absurd geometry).
    #[error("E_RECORD_INPUT: {0}")]
    Input(String),
}

impl RecordError {
    /// Stable token for operator-facing state.
    pub fn kind_token(&self) -> &'static str {
        match self {
            RecordError::Disk(_) => "E_DISK",
            RecordError::Aac(_) => "E_AAC_UNAVAILABLE",
            RecordError::Input(_) => "E_RECORD_INPUT",
        }
    }
}

/// Parameters for one recording file.
#[derive(Debug, Clone)]
pub struct RecordParams {
    /// `outputs.record.directory`: the file lands directly inside.
    pub directory: PathBuf,
    /// Show name — sanitized into the filename.
    pub show: String,
    /// Episode name — sanitized into the filename.
    pub episode: String,
    /// Recording-start timestamp (e.g. `20260914T120000Z`) — verbatim into
    /// the filename after sanitizing.
    pub start_timestamp: String,
    /// Program video geometry, for the `avc1` sample entry.
    pub width: u32,
    pub height: u32,
    /// Nominal frame rate (tail-sample duration + fragment sanity only;
    /// per-sample durations come from unit PTS).
    pub fps: u32,
    /// SPS + PPS from the stream's first keyframe
    /// (`EncodeSession::parameter_sets`), for the file's `avcC` box.
    /// VideoToolbox emits no parameter sets in-band — proven by dumping a
    /// keyframe (SEI + IDR only) — so they ride here rather than being
    /// parsed out of the units. Empty or mistyped sets are refused loudly.
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
}

/// Keep `[A-Za-z0-9._-]`; everything else becomes `_`, so a show title can
/// never escape the directory or break the extension.
pub fn sanitize_component(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "unnamed".into()
    } else {
        out
    }
}

/// `{show}_{episode}_{start-timestamp}.mp4`, sanitized.
pub fn recording_filename(params: &RecordParams) -> String {
    format!(
        "{}_{}_{}.mp4",
        sanitize_component(&params.show),
        sanitize_component(&params.episode),
        sanitize_component(&params.start_timestamp)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_sanitizes_path_escapes() {
        let p = RecordParams {
            directory: PathBuf::from("/tmp"),
            show: "../evil show!".into(),
            episode: "ep/01".into(),
            start_timestamp: "2026-09-14T12:00:00Z".into(),
            width: 640,
            height: 360,
            fps: 30,
            sps: vec![0x67],
            pps: vec![0x68],
        };
        let name = recording_filename(&p);
        assert!(!name.contains('/'));
        assert!(name.ends_with(".mp4"));
        assert_eq!(name, ".._evil_show__ep_01_2026-09-14T12_00_00Z.mp4");
    }

    #[test]
    fn error_tokens_are_stable() {
        assert_eq!(RecordError::Disk("x".into()).kind_token(), "E_DISK");
        assert_eq!(
            RecordError::Aac("x".into()).kind_token(),
            "E_AAC_UNAVAILABLE"
        );
        assert_eq!(
            RecordError::Input("x".into()).kind_token(),
            "E_RECORD_INPUT"
        );
    }
}
