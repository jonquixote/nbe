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
pub mod feed;
pub mod markers;
pub mod pool;
pub mod rtmp;
pub mod session;
pub mod stream;
pub mod tap_path;
pub mod thread;
pub mod writer;

pub use audio_tap::{AudioTap, DEFAULT_CAPACITY_SAMPLES};
pub use feed::{
    begin_tap_frame, end_take_on_chain_loss, end_tap_frame, handoff_record_frame,
    handoff_record_surface, restore_view, should_skip_record_frame, HandoffOutcome, TapLoan,
};

/// Build a take's surface pool at this geometry.
///
/// The size is `RECORD_CHANNEL_BOUND + 1`: one surface in flight per channel
/// slot, plus the one the compositor is drawing into. Decided in one place so
/// the pool cannot drift from the channel it feeds — a pool smaller than the
/// channel would shed frames the channel had room for, and a larger one would
/// hold VRAM that can never be in flight.
pub fn zerocopy_pool(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<nbe_decode::zerocopy::SurfacePool, nbe_decode::zerocopy::ZeroCopyError> {
    nbe_decode::zerocopy::SurfacePool::new(device, width, height, thread::RECORD_CHANNEL_BOUND + 1)
}

/// Build a take's SHARED surface pool at View geometry (Gate G1).
///
/// Sized [`pool::shared_pool_size`]: each consumer's queue plus the surface
/// its thread is encoding, plus the drawn one. Used by `record.start`; a
/// stream that starts while a take is live shares the take's pool.
pub fn shared_zerocopy_pool(
    device: &wgpu::Device,
) -> Result<nbe_decode::zerocopy::SurfacePool, nbe_decode::zerocopy::ZeroCopyError> {
    nbe_decode::zerocopy::SurfacePool::new(
        device,
        crate::render::VIEW_W,
        crate::render::VIEW_H,
        pool::shared_pool_size(thread::RECORD_CHANNEL_BOUND, stream::STREAM_CHANNEL_BOUND),
    )
}

/// Build the stream's own surface pool at View geometry, sized
/// [`pool::stream_pool_size`]. Built by `stream.start` (the chain probe IS
/// this build) and owned by the session — never by the render loop.
pub fn stream_zerocopy_pool(
    device: &wgpu::Device,
) -> Result<nbe_decode::zerocopy::SurfacePool, nbe_decode::zerocopy::ZeroCopyError> {
    nbe_decode::zerocopy::SurfacePool::new(
        device,
        crate::render::VIEW_W,
        crate::render::VIEW_H,
        pool::stream_pool_size(stream::STREAM_CHANNEL_BOUND),
    )
}
pub use session::{encoder_available, set_force_no_encoder, RecordSession, SessionError};
pub use thread::{
    await_done, ControlMsg, RecordMsg, SessionResult, ThreadArgs, RECORD_CHANNEL_BOUND,
    RECORD_STOP_TIMEOUT,
};
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
/// so the 1 Hz telemetry tick does not fork `df` every second; failures of
/// the `df` fork itself are cached for [`SPACE_CACHE_NEG_TTL`] (a sick disk
/// must not cost a fork per tick either); the probe still runs on every call
/// (one file create+remove, no fork). The `df` `Available` column is located
/// by header index, not by a fixed ordinal, so macOS (`Available`) and GNU
/// (`Available` or `Avail`) headers both parse. Both caches are bounded to
/// [`SPACE_CACHE_MAX_ENTRIES`] entries (oldest evicted), so a control plane
/// cycling directories cannot grow them without limit. Telemetry degrades any
/// refusal to `0.0` via [`crate::telemetry::record_space_mib_for`].
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
    if let Some(err) = space_neg_cache_get(dir) {
        return Err(RecordError::Disk(err));
    }
    match df_available_mib(dir) {
        Ok(mib) => {
            space_cache_put(dir, mib);
            Ok(mib)
        }
        Err(e) => {
            // Cache the fork failure itself (backoff): the message is the
            // refusal a tick would otherwise re-fork to rediscover.
            space_neg_cache_put(dir, e.to_string());
            Err(e)
        }
    }
}

/// TTL for the [`available_space_mib`] success cache: long enough that the
/// telemetry tick never forks `df` more than ~once per interval, short enough
/// that a filling disk shows up while it still matters.
const SPACE_CACHE_TTL: Duration = Duration::from_secs(5);

/// TTL for the `df`-failure backoff: a sick disk reports its cached refusal
/// instead of paying a fork per tick. Same order as the success TTL so a
/// recovered disk shows up just as fast.
const SPACE_CACHE_NEG_TTL: Duration = Duration::from_secs(5);

/// Bound on each space-cache map: oldest entry evicted past it.
const SPACE_CACHE_MAX_ENTRIES: usize = 64;

fn space_cache() -> &'static Mutex<HashMap<PathBuf, (Instant, f64)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Instant, f64)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn space_neg_cache() -> &'static Mutex<HashMap<PathBuf, (Instant, String)>> {
    static NEG_CACHE: OnceLock<Mutex<HashMap<PathBuf, (Instant, String)>>> = OnceLock::new();
    NEG_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Evict the oldest entry when a cache map would grow past its bound.
fn evict_oldest<K: Clone + Eq + std::hash::Hash, V>(map: &mut HashMap<K, (Instant, V)>) {
    if map.len() < SPACE_CACHE_MAX_ENTRIES {
        return;
    }
    let oldest = map
        .iter()
        .min_by_key(|(_, (at, _))| *at)
        .map(|(k, _)| k.clone());
    if let Some(k) = oldest {
        map.remove(&k);
    }
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
    evict_oldest(&mut guard);
    guard.insert(dir.to_path_buf(), (Instant::now(), mib));
}

fn space_neg_cache_get(dir: &Path) -> Option<String> {
    let guard = space_neg_cache().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get(dir)
        .filter(|(at, _)| at.elapsed() < SPACE_CACHE_NEG_TTL)
        .map(|(_, msg)| msg.clone())
}

fn space_neg_cache_put(dir: &Path, msg: String) {
    let mut guard = space_neg_cache().lock().unwrap_or_else(|e| e.into_inner());
    evict_oldest(&mut guard);
    guard.insert(dir.to_path_buf(), (Instant::now(), msg));
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

    #[test]
    fn space_caches_evict_oldest_past_the_bound() {
        // Direct cache-behavior pin: filling past the bound keeps the maps at
        // the bound and drops the oldest entry (no unbounded growth when the
        // control plane cycles directories).
        {
            let mut guard = space_cache().lock().unwrap_or_else(|e| e.into_inner());
            guard.clear();
            for n in 0..(SPACE_CACHE_MAX_ENTRIES + 10) {
                evict_oldest(&mut guard);
                guard.insert(
                    PathBuf::from(format!("/tmp/nbe-cache-{n}")),
                    (Instant::now(), 1.0),
                );
            }
            assert_eq!(guard.len(), SPACE_CACHE_MAX_ENTRIES);
            assert!(
                guard.get(&PathBuf::from("/tmp/nbe-cache-0")).is_none(),
                "oldest entry must evict first"
            );
            guard.clear();
        }
        {
            let mut guard = space_neg_cache().lock().unwrap_or_else(|e| e.into_inner());
            guard.clear();
            for n in 0..(SPACE_CACHE_MAX_ENTRIES + 10) {
                evict_oldest(&mut guard);
                guard.insert(
                    PathBuf::from(format!("/tmp/nbe-neg-{n}")),
                    (Instant::now(), "sick".into()),
                );
            }
            assert_eq!(guard.len(), SPACE_CACHE_MAX_ENTRIES);
            assert!(
                guard.get(&PathBuf::from("/tmp/nbe-neg-0")).is_none(),
                "oldest negative entry must evict first"
            );
            guard.clear();
        }
    }

    #[test]
    fn negative_cache_backs_off_within_ttl_and_expires_after() {
        // A cached df failure serves the refusal without a new fork while
        // fresh, and stops serving once stale (recovery is observable).
        let dir = PathBuf::from("/tmp/nbe-neg-backoff-probe");
        space_neg_cache_put(&dir, "sick disk".into());
        assert_eq!(
            space_neg_cache_get(&dir).as_deref(),
            Some("sick disk"),
            "fresh failure must back off"
        );
        {
            let mut guard = space_neg_cache().lock().unwrap_or_else(|e| e.into_inner());
            guard.insert(
                dir.clone(),
                (
                    Instant::now() - SPACE_CACHE_NEG_TTL - Duration::from_secs(1),
                    "sick disk".into(),
                ),
            );
        }
        assert!(
            space_neg_cache_get(&dir).is_none(),
            "stale failure must expire so a recovered disk is re-probed"
        );
        space_neg_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}
