//! Record session glue (Prompt 09 WU8, SPEC §16.14).
//!
//! Lifecycle owner between the directive path and the file writer:
//! `record.start` opens a [`RecordSession`], `record.stop` / `show.stop`
//! quiescence finishes it synchronously so the file + always-sidecar are
//! complete BEFORE `apply()` emits the ack (SPEC §5.9.5: the ack is honest
//! only after the effect is real).
//!
//! ## Why plain data (no live encoder/writer held)
//!
//! The live recording objects are `!Send`: `EncodeSession` holds a
//! `CFRetained<VTCompressionSession>` and `RecordingWriter` holds an
//! `AacEncoder` (`*mut OpaqueAudioConverter`), and `EngineState` must stay
//! `Send + Sync` (the runtime tasks capture it). Claiming otherwise would
//! need `unsafe impl`s in the encoder crates — out of scope and unsound to
//! rush. So the session held in engine state is plain data: naming, geometry,
//! the buffered units/audio, and the stream's parameter sets once the loop
//! feed captures them. `finish` runs the existing one-shot
//! [`write_recording`](crate::record::write_recording) shape over the
//! buffers. The streaming record thread (drain-tap + incremental fragments)
//! is a later work unit; until then buffering is bounded loudly (see caps
//! below), never silent.
//!
//! ## One encoder, one geometry (unification)
//!
//! The session NEVER opens an encoder: `open` validates preconditions,
//! reserves the output path, and stores naming + geometry only, with
//! `parameter_sets` EMPTY. The loop feed owns the ONE live `EncodeSession`
//! (opened at `VIEW_W/H` on the first fed frame) and captures the stream's
//! real SPS/PPS from its first keyframe into the session via
//! [`RecordSession::set_parameter_sets`]. The writer needs those sets for its
//! `avcC` box (VideoToolbox emits none in-band). There is no throwaway
//! black-frame encode anywhere: the recording starts with real content,
//! never black, and never at a second geometry.
//!
//! ## Seams and defined behavior
//!
//! * [`set_force_no_encoder`] forces the unavailable path exactly as a
//!   machine with no hardware encoder behaves — consulted by the loop feed
//!   before it touches hardware (the directive path touches no encoder at
//!   all), never a CPU fallback.
//! * [`RecordSession::open_synthetic`] is the hardware-free seam for tests: a
//!   session from caller-supplied sets with no hardware touched (AAC is still
//!   required at `finish`, when the real writer runs).
//! * Defined behavior: a second `record.start` while `Recording` is refused
//!   upstream with `E_FORBIDDEN_STATE` and preserves the live session; the
//!   chapter list still resets (WU5: every start attempt while running opens a
//!   fresh take).
//! * Stopping with no frames at all is `E_RECORD_INPUT`, mirroring
//!   [`write_recording`](crate::record::write_recording)'s "no video units"
//!   refusal — loudly, never a 0-byte file masquerading as a recording.
//!   Finishing before any keyframe exposed parameter sets is likewise
//!   `E_RECORD_INPUT`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::encode::EncodedUnit;
use crate::record::RecordError;
use crate::record::RecordParams;

/// Buffered-video cap, mirroring the writer's own pre-IDR window: a stream
/// with no IDR in ~20 s at 30 fps is broken input, not a slow start.
const MAX_PENDING_UNITS: usize = 600;
/// Buffered-audio cap: 60 s of stereo f32. Test-scale content passes; anything
/// beyond is the streaming thread's job (a later work unit), refused loudly.
const MAX_PENDING_AUDIO_SAMPLES: usize = 48_000 * 2 * 60;

/// Forced-unavailable seam (tests only): when set, the loop feed treats the
/// hardware encoder as absent without touching VideoToolbox. The directive
/// path never consults it — `open` touches no encoder at all.
static FORCE_NO_ENCODER: AtomicBool = AtomicBool::new(false);

/// Force (or release) the no-encoder path. Test seam only.
pub fn set_force_no_encoder(force: bool) {
    FORCE_NO_ENCODER.store(force, Ordering::SeqCst);
}

/// The seam state: true while forced-unavailable. The loop feed consults this
/// BEFORE attempting its encoder open, so forced-unavailable behaves exactly
/// like missing hardware without an open attempt.
pub fn force_no_encoder() -> bool {
    FORCE_NO_ENCODER.load(Ordering::SeqCst)
}

/// True unless the seam forces otherwise and a hardware H.264 encoder answers.
pub fn encoder_available() -> bool {
    if FORCE_NO_ENCODER.load(Ordering::SeqCst) {
        return false;
    }
    crate::encode::is_available()
}

/// Opening a record session fails loudly, with stable tokens.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// No hardware encoder (seam-forced or genuinely absent): `E_NO_HARDWARE_ENCODER`.
    #[error("E_NO_HARDWARE_ENCODER: {0}")]
    NoEncoder(String),
    /// Writer/file failure underneath (carries `E_DISK` / `E_AAC_UNAVAILABLE` /
    /// `E_RECORD_INPUT` verbatim).
    #[error(transparent)]
    Record(#[from] RecordError),
}

/// An open recording (plain data — see module docs): naming, geometry, the
/// buffered content awaiting `finish`, and the stream's parameter sets once
/// the loop feed captures them from its live encoder's first keyframe.
/// Deliberately `Send + Sync` so engine state can hold it.
#[derive(Debug)]
pub struct RecordSession {
    directory: PathBuf,
    show: String,
    episode: String,
    start_timestamp: String,
    width: u32,
    height: u32,
    fps: u32,
    /// The stream's real SPS/PPS, captured by the loop feed from the live
    /// encoder's first keyframe. `None` until then — `finish` refuses loudly
    /// without them, exactly as it does without video.
    parameter_sets: Option<(Vec<u8>, Vec<u8>)>,
    pending_video: Vec<EncodedUnit>,
    pending_audio: Vec<f32>,
    output_path: PathBuf,
}

impl RecordSession {
    /// Open a session in `directory`: validates nothing but the path —
    /// creates it, reserves the output filename — and flips the caller to
    /// `Recording`. No encoder is touched here (the handler stays
    /// non-blocking); the loop feed owns the ONE live encoder and fills
    /// `parameter_sets` from its first keyframe.
    ///
    /// Six args because the directive path has six independent metadata
    /// fields (naming + geometry + rate); bundling them would just move the
    /// struct boundary without removing a field.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        directory: &Path,
        show: &str,
        episode: &str,
        start_timestamp: &str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, RecordError> {
        std::fs::create_dir_all(directory).map_err(|e| RecordError::Disk(e.to_string()))?;
        let params = RecordParams {
            directory: directory.to_path_buf(),
            show: show.to_string(),
            episode: episode.to_string(),
            start_timestamp: start_timestamp.to_string(),
            width,
            height,
            fps,
            sps: Vec::new(),
            pps: Vec::new(),
        };
        let output_path = directory.join(crate::record::recording_filename(&params));
        Ok(Self {
            directory: directory.to_path_buf(),
            show: show.to_string(),
            episode: episode.to_string(),
            start_timestamp: start_timestamp.to_string(),
            width,
            height,
            fps,
            parameter_sets: None,
            pending_video: Vec::new(),
            pending_audio: Vec::new(),
            output_path,
        })
    }

    /// Hardware-free seam (tests only): a session from caller-supplied
    /// parameter sets with no hardware touched. `finish` still runs the real
    /// writer (so AAC is required there).
    pub fn open_synthetic(params: &RecordParams) -> Result<Self, RecordError> {
        std::fs::create_dir_all(&params.directory).map_err(|e| RecordError::Disk(e.to_string()))?;
        let output_path = params
            .directory
            .join(crate::record::recording_filename(params));
        Ok(Self {
            directory: params.directory.clone(),
            show: params.show.clone(),
            episode: params.episode.clone(),
            start_timestamp: params.start_timestamp.clone(),
            width: params.width,
            height: params.height,
            fps: params.fps,
            parameter_sets: Some((params.sps.clone(), params.pps.clone())),
            pending_video: Vec::new(),
            pending_audio: Vec::new(),
            output_path,
        })
    }

    /// The stream's parameter sets, if the loop feed has captured them yet.
    pub fn parameter_sets(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        self.parameter_sets.clone()
    }

    /// Store the stream's real SPS/PPS, captured by the loop feed from the
    /// live encoder's first keyframe. First capture wins: the sets are
    /// stream parameters, identical on every keyframe.
    pub fn set_parameter_sets(&mut self, sps: Vec<u8>, pps: Vec<u8>) {
        if self.parameter_sets.is_none() {
            self.parameter_sets = Some((sps, pps));
        }
    }

    /// Nominal frame rate, for the loop feed's encoder open.
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// The record target directory this session writes into.
    pub fn record_dir(&self) -> &Path {
        &self.directory
    }

    /// The reserved output path. The file materializes at `finish`.
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    /// Buffer one encoded access unit toward the file.
    pub fn push_video(&mut self, unit: &EncodedUnit) -> Result<(), RecordError> {
        if self.pending_video.len() >= MAX_PENDING_UNITS {
            return Err(RecordError::Input(format!(
                "{} buffered units with no finish; refusing runaway buffer",
                self.pending_video.len()
            )));
        }
        self.pending_video.push(EncodedUnit {
            data: unit.data.clone(),
            pts_seconds: unit.pts_seconds,
            is_keyframe: unit.is_keyframe,
        });
        Ok(())
    }

    /// Buffer interleaved stereo f32 toward the file (never dropped: buffered
    /// audio is content, not silence).
    pub fn push_audio(&mut self, pcm_f32: &[f32]) -> Result<(), RecordError> {
        if self.pending_audio.len() + pcm_f32.len() > MAX_PENDING_AUDIO_SAMPLES {
            return Err(RecordError::Input(format!(
                "buffered audio exceeds {} samples; the streaming record thread (later work unit) owns longer takes",
                MAX_PENDING_AUDIO_SAMPLES
            )));
        }
        self.pending_audio.extend_from_slice(pcm_f32);
        Ok(())
    }

    /// Finish synchronously: run the buffered content through the real writer
    /// (file + always-sidecar) and return the file path. Refuses loudly with
    /// no video (`E_RECORD_INPUT`) or with video but no captured parameter
    /// sets yet (no keyframe observed — also `E_RECORD_INPUT`).
    pub fn finish(self) -> Result<PathBuf, RecordError> {
        if self.pending_video.is_empty() {
            return Err(RecordError::Input("no video units".into()));
        }
        let Some((sps, pps)) = self.parameter_sets else {
            return Err(RecordError::Input(
                "no parameter sets captured (no keyframe observed)".into(),
            ));
        };
        let params = RecordParams {
            directory: self.directory,
            show: self.show,
            episode: self.episode,
            start_timestamp: self.start_timestamp,
            width: self.width,
            height: self.height,
            fps: self.fps,
            sps,
            pps,
        };
        crate::record::write_recording(&self.pending_video, &self.pending_audio, &params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_seam_reports_unavailable_without_touching_hardware() {
        set_force_no_encoder(true);
        assert!(!encoder_available());
        set_force_no_encoder(false);
    }
}
