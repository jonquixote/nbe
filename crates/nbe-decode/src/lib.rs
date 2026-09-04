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
    kCVPixelBufferPixelFormatTypeKey, kCVReturnSuccess, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetDataSize, CVPixelBufferGetHeight,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use std::path::Path;
use thiserror::Error;

/// `kCVPixelFormatType_32BGRA`, the format the reader is asked for.
const PIXEL_FORMAT_32BGRA: u32 = 0x42475241; // 'BGRA'

/// The largest frame this decoder will accept, per side.
///
/// Show media is attacker-influenced input (SPEC §10.7 assumes hostile
/// attention), and the pixel loop below reads through a raw pointer. A frame
/// larger than this is refused rather than trusted: it is far beyond any
/// house format, and the ceiling keeps the byte arithmetic provably in range.
const MAX_FRAME_DIMENSION: u32 = 16384;

/// Bytes one RGBA frame needs, or `None` if the dimensions are implausible or
/// the arithmetic would overflow.
///
/// Split out so the bound is testable without a decoder.
fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 || width > MAX_FRAME_DIMENSION || height > MAX_FRAME_DIMENSION {
        return None;
    }
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("no video track in {0}")]
    NoVideoTrack(String),
    #[error("cannot open {path}: {reason}")]
    Open { path: String, reason: String },
    #[error("decode failed for {path}: {reason}")]
    Failed { path: String, reason: String },
}

impl DecodeError {
    /// A stable, path-free token for this failure.
    ///
    /// The full message names a filesystem path and a platform error, which
    /// belong in the engine's own log. What crosses the render channel and
    /// ends up in operator-facing state is this token: enough to tell the
    /// classes of failure apart, without leaking the render node's layout.
    pub fn kind_token(&self) -> &'static str {
        match self {
            DecodeError::NoVideoTrack(_) => "noVideoTrack",
            DecodeError::Open { .. } => "openFailed",
            DecodeError::Failed { .. } => "decodeFailed",
        }
    }
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

            // Everything below reads through a raw pointer, so every
            // assumption the loop depends on is checked first rather than
            // trusted from the media.

            // 1. The format must be what we asked for. A different layout
            //    (planar YUV, say) has a different meaning for "4 bytes per
            //    pixel", and reading it as BGRA would walk off the plane.
            let format = CVPixelBufferGetPixelFormatType(&image);
            if format != PIXEL_FORMAT_32BGRA {
                return Err(DecodeError::Failed {
                    path: self.path.clone(),
                    reason: format!("expected 32BGRA pixel buffers, got format {format:#010x}"),
                });
            }

            // 2. The dimensions must be plausible and the allocation must not
            //    overflow.
            let Some(len) = rgba_byte_len(width, height) else {
                return Err(DecodeError::Failed {
                    path: self.path.clone(),
                    reason: format!("implausible frame dimensions {width}x{height}"),
                });
            };

            // 3. The lock must succeed. Reading the base address of a buffer
            //    that failed to lock is undefined behaviour, not an empty
            //    frame.
            let lock = CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly);
            if lock != kCVReturnSuccess {
                return Err(DecodeError::Failed {
                    path: self.path.clone(),
                    reason: format!("could not lock pixel buffer (CVReturn {lock})"),
                });
            }
            // From here to the unlock, every exit must unlock: the guard does
            // it on drop so an early return or a panic cannot leak the lock.
            let _unlock = PixelBufferLock { buffer: &image };

            let base = CVPixelBufferGetBaseAddress(&image);
            let stride = CVPixelBufferGetBytesPerRow(&image);
            let data_size = CVPixelBufferGetDataSize(&image);
            let row_bytes = width as usize * 4;

            // 4. The buffer must actually contain the bytes the loop will
            //    read: a full row per row, and the last row must fit.
            let last_byte = stride
                .checked_mul(height.saturating_sub(1) as usize)
                .and_then(|off| off.checked_add(row_bytes));
            let readable = !base.is_null()
                && stride >= row_bytes
                && matches!(last_byte, Some(needed) if needed <= data_size);
            if !readable {
                return Err(DecodeError::Failed {
                    path: self.path.clone(),
                    reason: format!(
                        "pixel buffer too small for {width}x{height}: stride {stride}, data size {data_size}"
                    ),
                });
            }

            let mut rgba = vec![0u8; len];
            let src = base as *const u8;
            for row in 0..height as usize {
                for col in 0..width as usize {
                    // In range by the checks above: row < height, col < width,
                    // stride >= width*4, and stride*(height-1)+width*4 fits
                    // inside the buffer's data size.
                    let s = src.add(row * stride + col * 4);
                    let d = (row * width as usize + col) * 4;
                    // BGRA (what we asked the reader for) → RGBA.
                    rgba[d] = *s.add(2);
                    rgba[d + 1] = *s.add(1);
                    rgba[d + 2] = *s;
                    rgba[d + 3] = *s.add(3);
                }
            }
            drop(_unlock);

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

/// Unlocks a pixel buffer when dropped, so no path between lock and unlock
/// can leak the lock — including an early return or a panic.
struct PixelBufferLock<'a> {
    buffer: &'a objc2_core_video::CVPixelBuffer,
}

impl Drop for PixelBufferLock<'_> {
    fn drop(&mut self) {
        unsafe {
            CVPixelBufferUnlockBaseAddress(self.buffer, CVPixelBufferLockFlags::ReadOnly);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_byte_len_refuses_overflow_and_absurd_dimensions() {
        assert_eq!(rgba_byte_len(1920, 1080), Some(1920 * 1080 * 4));
        assert_eq!(rgba_byte_len(0, 1080), None, "zero width");
        assert_eq!(rgba_byte_len(1920, 0), None, "zero height");
        assert_eq!(
            rgba_byte_len(u32::MAX, u32::MAX),
            None,
            "an overflowing product must not become a small allocation"
        );
        assert_eq!(
            rgba_byte_len(MAX_FRAME_DIMENSION + 1, 8),
            None,
            "beyond the accepted ceiling"
        );
        assert!(rgba_byte_len(MAX_FRAME_DIMENSION, MAX_FRAME_DIMENSION).is_some());
    }

    #[test]
    fn clip_and_loop_selection_are_pure() {
        assert_eq!(clip_source_index(10, 10, 5), Some(0));
        assert_eq!(clip_source_index(14, 10, 5), Some(4));
        assert_eq!(clip_source_index(15, 10, 5), None);
        assert_eq!(loop_source_index(15, 10, 5), Some(0));
        assert_eq!(loop_source_index(15, 10, 0), None);
    }
}
