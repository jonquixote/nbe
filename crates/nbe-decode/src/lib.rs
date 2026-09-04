//! nbe-decode: hardware video decode (Prompt 05 Step 2).
//!
//! Decode runs through AVFoundation's `AVAssetReader`, which is VideoToolbox
//! backed: H.264 and ProRes are decoded by the hardware decoder, not in
//! software. Frames arrive as `CVPixelBuffer`s carrying a presentation time,
//! which this module turns into a **presentation index** — the compositor
//! selects frames by index, never by wall time (SPEC §12.1).
//!
//! Where this runs matters more than how fast it is: every function here is
//! called at load, arm, or read-ahead time, on a decode thread. The render
//! loop never calls into this module (SPEC §7.13).

#![allow(unsafe_code)]
// The workspace denies `unsafe` (see the root Cargo.toml). This module is the
// single exception: every Objective-C message send into AVFoundation,
// CoreMedia, and CoreVideo is `unsafe` by construction, and hardware decode is
// not reachable from Rust without it. The unsafety is confined here — the
// types this module hands out (`DecodedFrame`, `AssetProbe`) are plain data.

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_av_foundation::{
    AVAsset, AVAssetReader, AVAssetReaderTrackOutput, AVAssetTrack, AVMediaTypeVideo, AVURLAsset,
};
use objc2_core_video::{
    kCVPixelBufferPixelFormatTypeKey, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use std::path::Path;
use thiserror::Error;

/// `kCVPixelFormatType_32BGRA`, the format the reader is asked for.
const PIXEL_FORMAT_32BGRA: u32 = 0x42475241; // 'BGRA'

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("no video track in {0}")]
    NoVideoTrack(String),
    #[error("cannot open {path}: {reason}")]
    Open { path: String, reason: String },
    #[error("decode failed for {path}: {reason}")]
    Failed { path: String, reason: String },
}

/// One decoded frame, addressed by its presentation index.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// Index in presentation order, from the source's own frame rate.
    pub index: u64,
    pub width: u32,
    pub height: u32,
    /// RGBA8, tightly packed.
    pub rgba: Vec<u8>,
    /// Presentation timestamp in seconds.
    pub pts_seconds: f64,
}

/// What a decode pass learned about an asset. Preflight consumes this
/// (SPEC §19.2); the engine uses the frame count and rate.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetProbe {
    pub width: u32,
    pub height: u32,
    /// Nominal frame rate as declared by the track.
    pub nominal_frame_rate: f32,
    /// Frames actually decoded.
    pub frame_count: u64,
    /// True when every inter-frame delta matched within tolerance (AC-3).
    pub cfr: bool,
    /// The measured frame rate from the presentation timestamps.
    pub measured_frame_rate: f64,
    pub has_alpha: bool,
}

/// A decode session over one asset.
///
/// Sessions are a bounded resource: VideoToolbox limits how many can be
/// active at once, which SPEC §24 names as a high-severity risk. The pool in
/// `session.rs` owns the cap; this type owns one session.
pub struct DecodeSession {
    reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderTrackOutput>,
    path: String,
    nominal_frame_rate: f32,
    next_index: u64,
    started: bool,
}

impl DecodeSession {
    /// Open an asset and prepare to read its video track from the start.
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        let path_str = path.display().to_string();
        unsafe {
            let ns_path = NSString::from_str(&path_str);
            let url = NSURL::fileURLWithPath(&ns_path);
            let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
            let asset_ref: &AVAsset = &asset;

            // `loadTracksWithMediaType:completionHandler:` is the modern
            // form, but it is asynchronous and needs a run loop; preflight and
            // arm-time preload are synchronous by design. The deprecated
            // synchronous accessor is the right tool here.
            #[allow(deprecated)]
            let tracks = asset_ref.tracksWithMediaType(AVMediaTypeVideo.expect("AVMediaTypeVideo"));
            let track: Retained<AVAssetTrack> = tracks
                .firstObject()
                .ok_or_else(|| DecodeError::NoVideoTrack(path_str.clone()))?;
            let nominal_frame_rate = track.nominalFrameRate();

            let reader = AVAssetReader::assetReaderWithAsset_error(asset_ref).map_err(|e| {
                DecodeError::Open {
                    path: path_str.clone(),
                    reason: e.localizedDescription().to_string(),
                }
            })?;

            // Ask the reader for BGRA: the conversion happens in the decode
            // pipeline, not in our code.
            // CFString and NSString are toll-free bridged; the pixel-format
            // key is published as a CFString.
            let key: &NSString =
                &*(kCVPixelBufferPixelFormatTypeKey as *const _ as *const NSString);
            let value = NSNumber::new_u32(PIXEL_FORMAT_32BGRA);
            let settings = NSDictionary::from_slices::<NSString>(&[key], &[&*value as &_]);
            let output = AVAssetReaderTrackOutput::initWithTrack_outputSettings(
                AVAssetReaderTrackOutput::alloc(),
                &track,
                Some(&settings),
            );
            output.setAlwaysCopiesSampleData(false);

            if !reader.canAddOutput(&output) {
                return Err(DecodeError::Open {
                    path: path_str,
                    reason: "reader rejected the track output".into(),
                });
            }
            reader.addOutput(&output);

            Ok(Self {
                reader,
                output,
                path: path_str,
                nominal_frame_rate,
                next_index: 0,
                started: false,
            })
        }
    }

    pub fn nominal_frame_rate(&self) -> f32 {
        self.nominal_frame_rate
    }

    /// Decode the next frame in presentation order, or `None` at end of
    /// stream. The index is assigned in decode order, which for these assets
    /// is presentation order.
    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, DecodeError> {
        unsafe {
            if !self.started {
                if !self.reader.startReading() {
                    return Err(DecodeError::Failed {
                        path: self.path.clone(),
                        reason: self
                            .reader
                            .error()
                            .map(|e| e.localizedDescription().to_string())
                            .unwrap_or_else(|| "startReading refused".into()),
                    });
                }
                self.started = true;
            }

            let Some(sample) = self.output.copyNextSampleBuffer() else {
                // End of stream, or a failure the reader recorded.
                if let Some(err) = self.reader.error() {
                    return Err(DecodeError::Failed {
                        path: self.path.clone(),
                        reason: err.localizedDescription().to_string(),
                    });
                }
                return Ok(None);
            };

            let pts = sample.presentation_time_stamp();
            let pts_seconds = if pts.timescale != 0 {
                pts.value as f64 / pts.timescale as f64
            } else {
                0.0
            };

            let Some(image) = sample.image_buffer() else {
                return Err(DecodeError::Failed {
                    path: self.path.clone(),
                    reason: "sample buffer carried no image".into(),
                });
            };

            let width = CVPixelBufferGetWidth(&image) as u32;
            let height = CVPixelBufferGetHeight(&image) as u32;
            CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly);
            let base = CVPixelBufferGetBaseAddress(&image);
            let stride = CVPixelBufferGetBytesPerRow(&image);
            let mut rgba = vec![0u8; (width * height * 4) as usize];
            if !base.is_null() {
                let src = base as *const u8;
                for row in 0..height as usize {
                    for col in 0..width as usize {
                        let s = src.add(row * stride + col * 4);
                        let d = (row * width as usize + col) * 4;
                        // BGRA (what we asked the reader for) → RGBA.
                        rgba[d] = *s.add(2);
                        rgba[d + 1] = *s.add(1);
                        rgba[d + 2] = *s;
                        rgba[d + 3] = *s.add(3);
                    }
                }
            }
            CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly);

            let index = self.next_index;
            self.next_index += 1;
            Ok(Some(DecodedFrame {
                index,
                width,
                height,
                rgba,
                pts_seconds,
            }))
        }
    }

    /// Decode every frame, returning them in presentation order. Used at load
    /// time for short loops and by preflight; long assets stream instead
    /// (SPEC §12.8).
    pub fn decode_all(&mut self, limit: usize) -> Result<Vec<DecodedFrame>, DecodeError> {
        let mut out = Vec::new();
        while out.len() < limit {
            match self.next_frame()? {
                Some(f) => out.push(f),
                None => break,
            }
        }
        Ok(out)
    }
}

/// Probe an asset: resolution, frame count, CFR, measured rate, alpha.
///
/// This is the decode-based half of preflight (SPEC §19, AC-3). It decodes
/// every frame, which is acceptable for a preflight pass and never happens on
/// air.
pub fn probe_asset(path: &Path, limit: usize) -> Result<AssetProbe, DecodeError> {
    let mut session = DecodeSession::open(path)?;
    let nominal_frame_rate = session.nominal_frame_rate();
    let frames = session.decode_all(limit)?;
    if frames.is_empty() {
        return Err(DecodeError::Failed {
            path: path.display().to_string(),
            reason: "no frames decoded".into(),
        });
    }

    let first = &frames[0];
    let width = first.width;
    let height = first.height;

    // CFR: every inter-frame delta equal within a tolerance well under half a
    // frame. A VFR source fails this (AC-3).
    let mut deltas: Vec<f64> = frames
        .windows(2)
        .map(|w| w[1].pts_seconds - w[0].pts_seconds)
        .filter(|d| *d > 0.0)
        .collect();
    let (cfr, measured_frame_rate) = if deltas.is_empty() {
        (true, nominal_frame_rate as f64)
    } else {
        deltas.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = deltas[0];
        let max = deltas[deltas.len() - 1];
        let mean = deltas.iter().sum::<f64>() / deltas.len() as f64;
        // Tolerance: a twentieth of a frame interval. Encoder rounding passes;
        // a genuinely variable cadence does not.
        let cfr = (max - min) <= mean / 20.0;
        (cfr, if mean > 0.0 { 1.0 / mean } else { 0.0 })
    };

    // Alpha: any pixel not fully opaque in the first frame.
    let has_alpha = first.rgba.as_chunks::<4>().0.iter().any(|px| px[3] != 255);

    Ok(AssetProbe {
        width,
        height,
        nominal_frame_rate,
        frame_count: frames.len() as u64,
        cfr,
        measured_frame_rate,
        has_alpha,
    })
}

/// Master-clock frame selection (SPEC §12.1, Prompt 05 Step 3).
///
/// A clip does not run on its own clock: the frame shown at master frame `f`
/// is a pure function of the show clock, which is what makes the compositor's
/// determinism property hold with video in the picture.
pub fn clip_source_index(
    master_frame: u64,
    item_start_frame: u64,
    frame_count: u64,
) -> Option<u64> {
    if frame_count == 0 || master_frame < item_start_frame {
        return None;
    }
    let elapsed = master_frame - item_start_frame;
    (elapsed < frame_count).then_some(elapsed)
}

/// Loop frame selection: `sourceIndex = (F - t0) mod P` (SPEC §12.1). A loop
/// boundary is computationally indistinguishable from any other frame — there
/// is no restart event, which is what AC-9 measures.
pub fn loop_source_index(master_frame: u64, t0: u64, period_frames: u64) -> Option<u64> {
    if period_frames == 0 {
        return None;
    }
    let elapsed = master_frame.saturating_sub(t0);
    Some(elapsed % period_frames)
}

/// Cadence hold pattern for a source slower than the house rate (SPEC §18,
/// AC-4). A 12 fps source at 30 fps house rate holds 2,3,2,3…
///
/// Returns the source index for each house frame across one full source
/// cycle, so a caller can assert the hold pattern exactly.
pub fn cadence_pattern(source_rate: u32, house_rate: u32, house_frames: u64) -> Vec<u64> {
    if source_rate == 0 || house_rate == 0 {
        return Vec::new();
    }
    (0..house_frames)
        .map(|f| f * source_rate as u64 / house_rate as u64)
        .collect()
}

/// The run lengths of a cadence pattern: `[2, 3, 2, 3]` for 12→30.
pub fn cadence_holds(pattern: &[u64]) -> Vec<u64> {
    let mut holds = Vec::new();
    let mut current = None;
    let mut run = 0u64;
    for idx in pattern {
        match current {
            Some(c) if c == *idx => run += 1,
            Some(_) => {
                holds.push(run);
                run = 1;
                current = Some(*idx);
            }
            None => {
                run = 1;
                current = Some(*idx);
            }
        }
    }
    if run > 0 {
        holds.push(run);
    }
    holds
}
