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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
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

/// Free space on the record target's volume, in MiB (SPEC §10.1:
/// `recordSpaceMib` reports free space; `E_DISK` on an unwritable target).
///
/// Space-check helper ONLY — the writer is untouched. Reads the real target
/// volume via `df -k <dir>` (pure std: no new dep, no `unsafe`, so the
/// workspace `unsafe_code = "deny"` lint stays green on macOS + Linux).
/// A file where the directory should be, a missing path, an unwritable
/// directory, or an unreadable `df` result all refuse as
/// [`RecordError::Disk`] (`E_DISK`), never panic.
///
/// Writability is a real probe, not permission bits: a zero-byte file is
/// created inside the directory and removed again. Bits lie — root bypasses
/// them, ACLs and read-only mounts ignore them — while a failed create is
/// the same refusal the recording itself would hit.
///
/// Cost control: successes are cached per directory for [`SPACE_CACHE_TTL`],
/// so the 1 Hz telemetry tick does not fork `df` every second; the probe
/// still runs on every call (one file create+remove, no fork). The `df`
/// `Available` column is located by header index, not by a fixed ordinal, so
/// macOS (`Available`) and GNU (`Available` or `Avail`) headers both parse.
/// Telemetry degrades any refusal to `0.0` via
/// [`crate::telemetry::record_space_mib_for`].
pub fn available_space_mib(dir: &Path) -> Result<f64, RecordError> {
    let meta = std::fs::metadata(dir)
        .map_err(|e| RecordError::Disk(format!("record target unreadable: {e}")))?;
    if !meta.is_dir() {
        return Err(RecordError::Disk(format!(
            "record target is not a directory: {}",
            dir.display()
        )));
    }
    probe_writable(dir)?;
    if let Some(cached) = space_cache_get(dir) {
        return Ok(cached);
    }
    let mib = df_available_mib(dir)?;
    space_cache_put(dir, mib);
    Ok(mib)
}

/// TTL for the [`available_space_mib`] success cache: long enough that the
/// telemetry tick never forks `df` more than ~once per interval, short enough
/// that a filling disk shows up while it still matters.
const SPACE_CACHE_TTL: Duration = Duration::from_secs(5);

fn space_cache() -> &'static Mutex<HashMap<PathBuf, (Instant, f64)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Instant, f64)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn space_cache_get(dir: &Path) -> Option<f64> {
    let guard = space_cache().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get(dir)
        .filter(|(at, _)| at.elapsed() < SPACE_CACHE_TTL)
        .map(|(_, mib)| *mib)
}

fn space_cache_put(dir: &Path, mib: f64) {
    let mut guard = space_cache().lock().unwrap_or_else(|e| e.into_inner());
    guard.insert(dir.to_path_buf(), (Instant::now(), mib));
}

/// Create-and-remove a zero-byte probe file: the only honest writability
/// check. Removal failures are ignored (a leftover probe does not block the
/// next call, which opens with `create(true)`).
fn probe_writable(dir: &Path) -> Result<(), RecordError> {
    let probe = dir.join(format!(".nbe-write-probe-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
        .map_err(|e| {
            RecordError::Disk(format!(
                "record target is not writable: {}: {e}",
                dir.display()
            ))
        })?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Parse the `Available` column (1K blocks) out of `df -k <dir>`, locating
/// the column by header name rather than ordinal.
fn df_available_mib(dir: &Path) -> Result<f64, RecordError> {
    let out = std::process::Command::new("df")
        .arg("-k")
        .arg(dir)
        .output()
        .map_err(|e| RecordError::Disk(format!("free-space query failed: {e}")))?;
    if !out.status.success() {
        return Err(RecordError::Disk(format!(
            "free-space query refused: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| RecordError::Disk("free-space query returned no output".to_string()))?;
    let header_cols: Vec<&str> = header.split_whitespace().collect();
    let avail_idx = header_cols
        .iter()
        .position(|c| *c == "Available" || *c == "Avail")
        .ok_or_else(|| {
            RecordError::Disk(format!(
                "free-space header has no Available column: {header}"
            ))
        })?;
    let last = lines
        .last()
        .ok_or_else(|| RecordError::Disk("free-space query returned no data line".to_string()))?;
    // The data line can carry fewer columns than the header when a mount
    // point contains whitespace; the Available value sits at the same offset
    // from the END in that case. Prefer the header index, fall back to the
    // from-the-end position.
    let cols: Vec<&str> = last.split_whitespace().collect();
    let avail = cols
        .get(avail_idx)
        .or_else(|| {
            let from_end = header_cols.len().checked_sub(avail_idx)?;
            cols.len().checked_sub(from_end).and_then(|i| cols.get(i))
        })
        .ok_or_else(|| RecordError::Disk(format!("free-space query unparseable: {last}")))?;
    let avail_kib: f64 = avail
        .parse()
        .map_err(|_| RecordError::Disk(format!("free-space query unparseable: {last}")))?;
    Ok(avail_kib / 1024.0)
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
