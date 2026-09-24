//! Hardware H.264 encode (Prompt 09 WU2, SPEC §9.2).
//!
//! VideoToolbox compression sessions, hardware encoders only. The shape
//! mirrors `DecodeSession` in `super` (then `lib.rs`): every Objective-C /
//! CoreFoundation / VideoToolbox call is `unsafe` by construction, so all of
//! it lives in this crate — the one crate the workspace allows `unsafe` in —
//! and the types handed out (`EncodedUnit`, `EncodeSession`) are plain data
//! plus one opaque session handle.
//!
//! Hardware-only, no CPU fallback anywhere:
//! - the session is created with
//!   `kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder`,
//!   so creation itself fails where no hardware encoder exists;
//! - after creation `kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder`
//!   is read back and the session is refused unless it is true;
//! - `open_with_options(..., force_software = true)` refuses immediately with
//!   [`EncodeError::NoHardwareEncoder`] without touching VideoToolbox. That
//!   flag is a test seam, not a selector: there is no software path to select.
//!
//! Frame behavior:
//! - presentation timestamps are `frame_index / fps`, strictly increasing;
//!   frame reordering is disabled (`AllowFrameReordering = false`), so decode
//!   order is display order and the PTS series the test asserts on is the one
//!   fed in;
//! - frame 0 is forced to an IDR (`ForceKeyFrame`), and `MaxKeyFrameInterval`
//!   is one second of frames, so the first second always carries a keyframe;
//! - a unit is a keyframe when its sample carries no `NotSync` attachment;
//! - input is tightly packed RGBA and is swizzled to BGRA on the way into the
//!   session pool's pixel buffers;
//! - parameter sets are NOT in-band: VideoToolbox emits slices only (SEI +
//!   IDR for a keyframe), and SPS/PPS live in the sample's format description
//!   (`CMVideoFormatDescription`). The output callback captures them from the
//!   first keyframe for the recording writer's `avcC` box; see
//!   [`EncodeSession::parameter_sets`].
//!
//! Floor: 640x360 is the smallest geometry the hardware encoder opens (see
//! [`is_available`]); narrower widths are refused by the hardware, which
//! surfaces here as [`EncodeError::NoHardwareEncoder`].

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    kCMSampleAttachmentKey_NotSync, kCMTimeInvalid, kCMVideoCodecType_H264, CMSampleBuffer, CMTime,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
};
use objc2_core_video::{
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA, kCVReturnSuccess, CVPixelBuffer,
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetDataSize,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
    kVTEncodeFrameOptionKey_ForceKeyFrame,
    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, VTCompressionSession,
    VTEncodeInfoFlags, VTSessionCopyProperty, VTSessionSetProperty,
};
use thiserror::Error;

/// One compressed access unit: the payload of a single output callback.
#[derive(Debug, Clone)]
pub struct EncodedUnit {
    /// Coded bytes for this frame (AVCC length-prefixed NALs).
    pub data: Vec<u8>,
    /// Presentation timestamp in seconds (`frame_index / fps`).
    pub pts_seconds: f64,
    /// True when the sample carries no `NotSync` attachment.
    pub is_keyframe: bool,
}

/// Encode failure.
///
/// There is exactly one class of failure by design: anything that stops the
/// stream — no hardware encoder on the machine, a refused geometry, a
/// VideoToolbox error mid-stream — means hardware encode is unavailable, and
/// there is no fallback to degrade to.
#[derive(Debug, Error)]
pub enum EncodeError {
    /// Hardware H.264 encode is unavailable, or the encode failed.
    #[error("E_NO_HARDWARE_ENCODER: {0}")]
    NoHardwareEncoder(String),
}

impl EncodeError {
    /// A stable token for this failure, mirroring `DecodeError::kind_token`.
    ///
    /// The full message carries geometry and platform status codes for the
    /// engine log; what crosses into operator-facing state is this token.
    pub fn kind_token(&self) -> &'static str {
        match self {
            EncodeError::NoHardwareEncoder(_) => "E_NO_HARDWARE_ENCODER",
        }
    }
}

fn no_hardware(detail: impl Into<String>) -> EncodeError {
    EncodeError::NoHardwareEncoder(detail.into())
}

/// State shared with the VideoToolbox output callback.
///
/// The callback runs on a VideoToolbox thread, so everything behind the
/// mutexes must be `Send + Sync` — plain data only, no session handles.
///
/// `units` is append-only: [`EncodeSession::encode_rgba`] reports newly
/// arrived units via the `delivered` cursor, and [`EncodeSession::finish`]
/// replays the whole stream. Nothing is ever removed, so a caller that only
/// reads `finish` still sees every frame — including the opening IDR, which
/// typically arrives while later frames are still being fed.
#[derive(Debug, Default)]
struct CallbackState {
    /// Every unit received, in callback (decode) order. Reordering is
    /// disabled on the session, so this is display order.
    units: Mutex<Vec<EncodedUnit>>,
    /// How many of `units` have been reported through `encode_rgba`.
    delivered: Mutex<usize>,
    /// First callback-side failure, if any. Surfaced by `finish` only when
    /// no units arrived at all.
    first_error: Mutex<Option<String>>,
    /// SPS + PPS captured from the first keyframe's format description
    /// (VideoToolbox emits no parameter sets in-band). `None` until a
    /// keyframe with a readable description arrives.
    param_sets: Mutex<Option<(Vec<u8>, Vec<u8>)>>,
}

fn record_callback_error(state: &CallbackState, detail: String) {
    let mut slot = state.first_error.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(detail);
    }
}

fn take_callback_error(state: &CallbackState) -> Option<String> {
    state
        .first_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// True when the sample carries no `NotSync` attachment.
///
/// `CMSampleBufferGetSampleAttachmentsArray` hands back one dictionary per
/// sample; absence of the whole array, an empty array, a missing entry, or
/// an explicit false all mean "sync sample", i.e. keyframe.
fn sample_is_keyframe(sample: &CMSampleBuffer) -> bool {
    unsafe {
        let Some(attachments) = sample.sample_attachments_array(false) else {
            return true;
        };
        if attachments.count() == 0 {
            return true;
        }
        let dict_ptr = attachments.value_at_index(0);
        if dict_ptr.is_null() {
            return true;
        }
        let dict = &*(dict_ptr as *const objc2_core_foundation::CFDictionary);
        let key = kCMSampleAttachmentKey_NotSync as *const CFString as *const c_void;
        if !dict.contains_ptr_key(key) {
            return true;
        }
        let mut value: *const c_void = std::ptr::null();
        if dict.value_if_present(key, &mut value) && !value.is_null() {
            return !(&*(value as *const CFBoolean)).value();
        }
        false
    }
}

/// Capture SPS + PPS from a keyframe sample's format description, once.
///
/// VideoToolbox emits no parameter sets in-band, so the recording writer's
/// `avcC` has no other source. Index 0 is the SPS, index 1 the PPS (verified
/// by NAL type below, not trusted by position). The returned pointers borrow
/// the format description, so the bytes are copied out immediately. A sample
/// whose description is unreadable is skipped silently: recording without
/// parameter sets is refused later, loudly, at the writer — never here with
/// half a header.
fn capture_parameter_sets(state: &CallbackState, sample: &CMSampleBuffer) {
    if state
        .param_sets
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
    {
        return;
    }
    let sets: Option<(Vec<u8>, Vec<u8>)> = unsafe {
        let desc = sample.format_description();
        let desc = match desc {
            Some(d) => d,
            None => return,
        };
        let read_set = |index: usize, want_type: u8| -> Option<Vec<u8>> {
            let mut ptr: *const u8 = std::ptr::null();
            let mut size: usize = 0;
            let mut count: usize = 0;
            let mut nalu_len: std::ffi::c_int = 0;
            let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                &desc,
                index,
                &mut ptr,
                &mut size,
                &mut count,
                &mut nalu_len,
            );
            if status != 0 || ptr.is_null() || size == 0 || size > 1 << 20 {
                return None;
            }
            let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
            if bytes.first().map(|b| b & 0x1F) != Some(want_type) {
                return None;
            }
            Some(bytes)
        };
        let sps = read_set(0, 7);
        let pps = read_set(1, 8);
        match (sps, pps) {
            (Some(s), Some(p)) => Some((s, p)),
            _ => None,
        }
    };
    if let Some(sets) = sets {
        *state.param_sets.lock().unwrap_or_else(|e| e.into_inner()) = Some(sets);
    }
}
/// The `VTCompressionOutputCallback` given to `VTCompressionSessionCreate`.
///
/// `refcon` is a borrowed `Arc<CallbackState>` (kept alive by the owning
/// [`EncodeSession`]); this function takes no ownership, it only locks,
/// copies the payload out, and pushes.
unsafe extern "C-unwind" fn encode_output_callback(
    refcon: *mut c_void,
    _source_frame: *mut c_void,
    status: i32,
    _info_flags: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    unsafe {
        if refcon.is_null() {
            return;
        }
        let state = &*(refcon as *const CallbackState);
        if status != 0 {
            record_callback_error(
                state,
                format!("compression callback reported OSStatus {status}"),
            );
            return;
        }
        if sample.is_null() {
            return;
        }
        let sample = &*sample;

        let pts = sample.presentation_time_stamp();
        let pts_seconds = if pts.timescale != 0 {
            pts.value as f64 / pts.timescale as f64
        } else {
            0.0
        };
        let is_keyframe = sample_is_keyframe(sample);

        // Parameter sets are not in-band (the payload above is slices
        // only), so capture them from the first keyframe's format
        // description for the recording writer's `avcC`. Once captured, or
        // on a non-keyframe, this is a single mutex check — the steady
        // state costs nothing.
        if is_keyframe {
            capture_parameter_sets(state, sample);
        }

        let Some(block) = sample.data_buffer() else {
            record_callback_error(state, "compressed sample carried no data buffer".into());
            return;
        };
        let len = block.data_length();
        if len == 0 {
            record_callback_error(
                state,
                "compressed sample carried an empty data buffer".into(),
            );
            return;
        }
        let mut data = vec![0u8; len];
        let dst = NonNull::new(data.as_mut_ptr() as *mut c_void)
            .expect("a freshly allocated buffer is non-null");
        if block.copy_data_bytes(0, len, dst) != 0 {
            record_callback_error(state, "could not copy compressed sample bytes".into());
            return;
        }
        state
            .units
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(EncodedUnit {
                data,
                pts_seconds,
                is_keyframe,
            });
    }
}

/// Unlocks a pixel buffer when dropped, so no path between lock and unlock
/// can leak the lock — including an early return or a panic. Write-side twin
/// of the `PixelBufferLock` in `super`.
struct PixelBufferWriteGuard<'a> {
    buffer: &'a CVPixelBuffer,
}

impl Drop for PixelBufferWriteGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            CVPixelBufferUnlockBaseAddress(self.buffer, CVPixelBufferLockFlags::empty());
        }
    }
}

/// A hardware H.264 encode session over one stream.
///
/// Sessions are a bounded resource (see `DecodeSession` in `super` for the
/// same warning on the decode side): create one, feed it, `finish` it. The
/// session is invalidated deterministically on `finish` and on drop, so the
/// [`is_available`] probe — which opens a session only to test that hardware
/// answers — never leaks one.
#[derive(Debug)]
pub struct EncodeSession {
    /// `None` once `finish` has invalidated the session; `Drop` invalidates
    /// whatever is still here.
    session: Option<CFRetained<VTCompressionSession>>,
    /// Kept alive for the session's lifetime: the output callback borrows it
    /// as its `refcon`.
    state: Arc<CallbackState>,
    width: u32,
    height: u32,
    fps: u32,
    frame_index: u64,
    /// The last caller-supplied PTS index ([`Self::encode_pixel_buffer_at`]),
    /// so a non-increasing one is refused before VideoToolbox sees it.
    last_pts_index: Option<u64>,
}

impl EncodeSession {
    /// Open a hardware H.264 encoder for `width`x`height` at `fps` with an
    /// average bit rate of `bitrate` bits per second.
    pub fn open(width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self, EncodeError> {
        Self::open_with_options(width, height, fps, bitrate, false)
    }

    /// Open with the software leg forced.
    ///
    /// `force_software = true` refuses immediately with
    /// [`EncodeError::NoHardwareEncoder`] without touching VideoToolbox. It
    /// is a test seam proving the refusal path behaves exactly as a machine
    /// with no hardware encoder would — never a CPU-fallback selector.
    pub fn open_with_options(
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        force_software: bool,
    ) -> Result<Self, EncodeError> {
        if force_software {
            return Err(no_hardware(
                "software encode refused: this encoder is hardware-only, no CPU fallback exists",
            ));
        }
        if width == 0 || height == 0 {
            return Err(no_hardware(format!(
                "refusing zero-sized encode {width}x{height}"
            )));
        }
        if width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(no_hardware(format!(
                "refusing implausible encode geometry {width}x{height}"
            )));
        }
        if fps == 0 || fps > i32::MAX as u32 {
            return Err(no_hardware(format!("refusing encode frame rate {fps}")));
        }
        if bitrate > i32::MAX as u32 {
            return Err(no_hardware(format!("refusing encode bit rate {bitrate}")));
        }
        unsafe { Self::create_hw(width, height, fps, bitrate) }
    }

    /// Create the VideoToolbox session, hardware-pinned. Caller has
    /// validated the geometry; every failure here is `NoHardwareEncoder`.
    unsafe fn create_hw(
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> Result<Self, EncodeError> {
        unsafe {
            let state = Arc::new(CallbackState::default());
            // Borrowed by the callback; the `Arc` itself lives in `Self`.
            let refcon = Arc::as_ptr(&state) as *mut c_void;

            // Encoder spec: hardware or nothing. Where no hardware encoder
            // can serve H.264 at this geometry, creation fails here.
            let spec = {
                let key: &CFString =
                    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder;
                let value: &CFType = CFBoolean::new(true);
                CFDictionary::<CFString, CFType>::from_slices(&[key], &[value])
            };
            // Source buffers: 32BGRA, matching the swizzle on feed.
            let source_attrs = {
                let key: &CFString = kCVPixelBufferPixelFormatTypeKey;
                let format = CFNumber::new_i32(kCVPixelFormatType_32BGRA as i32);
                let value: &CFType = &format;
                CFDictionary::<CFString, CFType>::from_slices(&[key], &[value])
            };

            let mut raw: *mut VTCompressionSession = std::ptr::null_mut();
            let status = VTCompressionSession::create(
                None,
                width as i32,
                height as i32,
                kCMVideoCodecType_H264,
                Some(spec.as_opaque()),
                Some(source_attrs.as_opaque()),
                None,
                Some(encode_output_callback),
                refcon,
                NonNull::new(&mut raw).expect("a stack out-pointer is non-null"),
            );
            if status != 0 {
                return Err(no_hardware(format!(
                    "VTCompressionSessionCreate for {width}x{height} failed (OSStatus {status}); no hardware H.264 encoder"
                )));
            }
            let session: CFRetained<VTCompressionSession> = CFRetained::from_raw(
                NonNull::new(raw).ok_or_else(|| no_hardware("session creation returned null"))?,
            );

            // Post-create verify: refuse unless a hardware encoder was
            // actually selected, even though creation already required one.
            if !session_using_hardware(&session)? {
                return Err(no_hardware(
                    "session is not using a hardware accelerated encoder; refusing (hardware-only)",
                ));
            }

            set_bool_property(
                &session,
                kVTCompressionPropertyKey_AllowFrameReordering,
                false,
            )?;
            set_number_property(
                &session,
                kVTCompressionPropertyKey_MaxKeyFrameInterval,
                fps as i32,
            )?;
            set_number_property(
                &session,
                kVTCompressionPropertyKey_AverageBitRate,
                bitrate as i32,
            )?;
            set_number_property(
                &session,
                kVTCompressionPropertyKey_ExpectedFrameRate,
                fps as i32,
            )?;

            let prepared = session.prepare_to_encode_frames();
            if prepared != 0 {
                return Err(no_hardware(format!(
                    "PrepareToEncodeFrames failed (OSStatus {prepared})"
                )));
            }

            Ok(Self {
                session: Some(session),
                state,
                width,
                height,
                fps,
                frame_index: 0,
                last_pts_index: None,
            })
        }
    }

    /// Feed one tightly packed RGBA frame. Returns the access units that
    /// arrived since the previous call — often empty, since encode runs
    /// asynchronously. Units are also retained: [`Self::finish`] replays the
    /// complete stream regardless of what was already reported here.
    pub fn encode_rgba(&mut self, rgba: &[u8]) -> Result<Vec<EncodedUnit>, EncodeError> {
        let expected = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| {
                no_hardware(format!(
                    "implausible encode geometry {}x{}",
                    self.width, self.height
                ))
            })?;
        if rgba.len() != expected {
            return Err(no_hardware(format!(
                "expected {expected} bytes of RGBA for {}x{}, got {}",
                self.width,
                self.height,
                rgba.len()
            )));
        }
        let Some(session) = self.session.as_ref() else {
            return Err(no_hardware("encode session is already finished"));
        };
        unsafe {
            feed_frame(
                session,
                rgba,
                self.width,
                self.height,
                self.frame_index,
                self.fps,
            )?;
        }
        self.frame_index += 1;
        Ok(take_new_units(&self.state))
    }

    /// Encode a `CVPixelBuffer` the caller already owns — **no pool fetch and
    /// no copy**.
    ///
    /// The zero-copy path's encode seam (ZERO-COPY Phase 3b, step 3), restored
    /// from the Phase 1 spike as production. [`Self::encode_rgba`] allocates a
    /// buffer from the session's pool and copies RGBA into it row by row,
    /// swizzling to BGRA as it goes; that copy is the whole cost this path
    /// exists to remove. Here the buffer IS the allocation the compositor drew
    /// into, so the encoder reads the compositor's pixels directly.
    ///
    /// Geometry and format are checked against the session rather than
    /// trusted: a buffer of the wrong shape would be *interpreted* rather than
    /// rejected, which is how a zero-copy path produces a plausible-looking
    /// corrupt file instead of an error.
    pub fn encode_pixel_buffer(
        &mut self,
        buffer: &CVPixelBuffer,
    ) -> Result<Vec<EncodedUnit>, EncodeError> {
        let Some(session) = self.session.as_ref() else {
            return Err(no_hardware("encode session is already finished"));
        };
        unsafe {
            check_zero_copy_buffer(buffer, self.width, self.height)?;
            submit_frame(session, buffer, self.frame_index, self.fps)?;
        }
        self.frame_index += 1;
        Ok(take_new_units(&self.state))
    }

    /// [`Self::encode_pixel_buffer`] with the PTS supplied by the caller:
    /// `pts_index / fps` seconds instead of the session's own submit count.
    ///
    /// For a live output whose frames can be shed before they reach the
    /// encoder. Counting submissions (the record path's timeline) makes every
    /// shed frame pull the video timeline one frame period behind the audio
    /// timeline, which counts samples the audio driver actually rendered; over
    /// a long stream that is unbounded A/V drift. Stamping each frame with its
    /// position on the show clock leaves a gap where a frame was shed —
    /// which is what happened — and keeps video and audio on one clock.
    ///
    /// `pts_index` must strictly increase across calls (VideoToolbox refuses
    /// a non-increasing PTS); a violation is refused here, loudly, before the
    /// encoder sees it. The first submission is still the forced IDR.
    pub fn encode_pixel_buffer_at(
        &mut self,
        buffer: &CVPixelBuffer,
        pts_index: u64,
    ) -> Result<Vec<EncodedUnit>, EncodeError> {
        if let Some(last) = self.last_pts_index {
            if pts_index <= last {
                return Err(no_hardware(format!(
                    "pts index {pts_index} does not follow {last}; refusing a non-increasing timeline"
                )));
            }
        }
        let Some(session) = self.session.as_ref() else {
            return Err(no_hardware("encode session is already finished"));
        };
        unsafe {
            check_zero_copy_buffer(buffer, self.width, self.height)?;
            submit_frame_at(session, buffer, pts_index, self.fps, self.frame_index == 0)?;
        }
        self.frame_index += 1;
        self.last_pts_index = Some(pts_index);
        Ok(take_new_units(&self.state))
    }

    /// Flush all pending frames and return every access unit of the stream,
    /// in PTS order — including units already reported by [`Self::encode_rgba`].
    pub fn finish(mut self) -> Result<Vec<EncodedUnit>, EncodeError> {
        if let Some(session) = self.session.take() {
            let status = unsafe { session.complete_frames(kCMTimeInvalid) };
            // Deterministic teardown before the retain is released.
            unsafe {
                session.invalidate();
            }
            if status != 0 {
                return Err(no_hardware(format!(
                    "flushing pending frames failed (OSStatus {status})"
                )));
            }
        }
        let mut units = all_units(&self.state);
        if units.is_empty() {
            return Err(match take_callback_error(&self.state) {
                Some(detail) => no_hardware(format!("encoder produced no access units ({detail})")),
                None => no_hardware("encoder produced no access units"),
            });
        }
        units.sort_by(|a, b| {
            a.pts_seconds
                .partial_cmp(&b.pts_seconds)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(units)
    }

    /// SPS + PPS captured from the first keyframe's format description, if
    /// one has arrived. The recording writer needs these for the file's
    /// `avcC` box (VideoToolbox emits no parameter sets in-band — see the
    /// module docs). `None` means no keyframe has been observed yet, or its
    /// description was unreadable; recording without them is refused loudly
    /// at the writer.
    pub fn parameter_sets(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        self.state
            .param_sets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// True when a hardware H.264 encoder answers: opens 640x360 and drops
    /// it. That geometry is the floor the hardware opens — the roundtrip
    /// runs at the smallest size the encoder accepts.
    pub fn is_available() -> bool {
        Self::open(640, 360, 30, 1_000_000).is_ok()
    }
}

/// Back-compat alias matching the test import path (`is_available` is also
/// an associated function; this free function is what `nbe_engine::encode`
/// re-exports).
pub fn is_available() -> bool {
    EncodeSession::is_available()
}

impl Drop for EncodeSession {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            unsafe {
                session.invalidate();
            }
        }
    }
}

/// Every unit received so far.
fn all_units(state: &CallbackState) -> Vec<EncodedUnit> {
    state
        .units
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Units that arrived since the last call, advancing the cursor.
fn take_new_units(state: &CallbackState) -> Vec<EncodedUnit> {
    let units = state.units.lock().unwrap_or_else(|e| e.into_inner());
    let mut delivered = state.delivered.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = units[*delivered..].to_vec();
    *delivered = units.len();
    fresh
}

/// Read back whether the session actually selected a hardware encoder.
unsafe fn session_using_hardware(
    session: &CFRetained<VTCompressionSession>,
) -> Result<bool, EncodeError> {
    unsafe {
        let key: &CFString = kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder;
        let mut raw: *const c_void = std::ptr::null();
        let status = VTSessionCopyProperty(session, key, None, &mut raw as *mut _ as *mut c_void);
        if status != 0 || raw.is_null() {
            return Err(no_hardware(format!(
                "could not verify hardware encoder selection (OSStatus {status})"
            )));
        }
        // Retained by the copy; dropped (released) at the end of this scope.
        let flag: CFRetained<CFBoolean> =
            CFRetained::from_raw(NonNull::new(raw as *mut CFBoolean).expect("null checked above"));
        Ok(flag.value())
    }
}

/// Set a boolean session property, failing as `NoHardwareEncoder`.
unsafe fn set_bool_property(
    session: &CFRetained<VTCompressionSession>,
    key: &CFString,
    value: bool,
) -> Result<(), EncodeError> {
    unsafe {
        let flag: &CFType = CFBoolean::new(value);
        let status = VTSessionSetProperty(session, key, Some(flag));
        if status != 0 {
            return Err(no_hardware(format!(
                "setting compression property failed (OSStatus {status})"
            )));
        }
        Ok(())
    }
}

/// Set a numeric session property, failing as `NoHardwareEncoder`.
unsafe fn set_number_property(
    session: &CFRetained<VTCompressionSession>,
    key: &CFString,
    value: i32,
) -> Result<(), EncodeError> {
    unsafe {
        let number = CFNumber::new_i32(value);
        let property: &CFType = &number;
        let status = VTSessionSetProperty(session, key, Some(property));
        if status != 0 {
            return Err(no_hardware(format!(
                "setting compression property failed (OSStatus {status})"
            )));
        }
        Ok(())
    }
}

/// Copy one RGBA frame into a session pool buffer (swizzled to BGRA) and
/// hand it to the encoder with PTS `frame_index / fps`.
unsafe fn feed_frame(
    session: &CFRetained<VTCompressionSession>,
    rgba: &[u8],
    width: u32,
    height: u32,
    frame_index: u64,
    fps: u32,
) -> Result<(), EncodeError> {
    unsafe {
        let pool = session
            .pixel_buffer_pool()
            .ok_or_else(|| no_hardware("compression session has no pixel buffer pool"))?;

        let mut raw: *mut CVPixelBuffer = std::ptr::null_mut();
        let created = CVPixelBufferPool::create_pixel_buffer(
            None,
            &pool,
            NonNull::new(&mut raw).expect("a stack out-pointer is non-null"),
        );
        if created != kCVReturnSuccess {
            return Err(no_hardware(format!(
                "pixel buffer pool refused a buffer (CVReturn {created})"
            )));
        }
        let buffer: CFRetained<CVPixelBuffer> = CFRetained::from_raw(
            NonNull::new(raw).ok_or_else(|| no_hardware("pool returned a null buffer"))?,
        );

        // The pool was asked for 32BGRA at creation; refuse anything else
        // rather than misinterpreting its planes.
        if CVPixelBufferGetPixelFormatType(&buffer) != kCVPixelFormatType_32BGRA {
            return Err(no_hardware("pixel buffer pool delivered a non-BGRA buffer"));
        }
        let pool_width = CVPixelBufferGetWidth(&buffer);
        let pool_height = CVPixelBufferGetHeight(&buffer);
        if pool_width != width as usize || pool_height != height as usize {
            return Err(no_hardware(format!(
                "pool buffer is {pool_width}x{pool_height}, session is {width}x{height}"
            )));
        }

        let locked = CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
        if locked != kCVReturnSuccess {
            return Err(no_hardware(format!(
                "could not lock pixel buffer (CVReturn {locked})"
            )));
        }
        // Every exit below unlocks, including an early return or a panic.
        let _unlock = PixelBufferWriteGuard { buffer: &buffer };

        let base = CVPixelBufferGetBaseAddress(&buffer);
        let stride = CVPixelBufferGetBytesPerRow(&buffer);
        let data_size = CVPixelBufferGetDataSize(&buffer);
        let row_bytes = pool_width * 4;
        let last_byte = stride
            .checked_mul(pool_height.saturating_sub(1))
            .and_then(|offset| offset.checked_add(row_bytes));
        if base.is_null()
            || stride < row_bytes
            || !matches!(last_byte, Some(needed) if needed <= data_size)
        {
            return Err(no_hardware(format!(
                "pixel buffer too small for {width}x{height}: stride {stride}, data size {data_size}"
            )));
        }

        // RGBA (caller) → BGRA (encoder). In range by the checks above.
        let src = rgba.as_ptr();
        let dst = base as *mut u8;
        for row in 0..pool_height {
            for col in 0..pool_width {
                let s = src.add((row * pool_width + col) * 4);
                let d = dst.add(row * stride + col * 4);
                *d = *s.add(2);
                *d.add(1) = *s.add(1);
                *d.add(2) = *s;
                *d.add(3) = *s.add(3);
            }
        }
        drop(_unlock);

        submit_frame(session, &buffer, frame_index, fps)
    }
}

/// Hand one `CVPixelBuffer` to VideoToolbox.
///
/// Extracted from [`feed_frame`] so the zero-copy path submits through the
/// SAME code — the PTS arithmetic, the forced IDR on frame 0, and the OSStatus
/// check are stream properties, not properties of how the pixels got there.
/// Two copies of this would be two chances for the paths to disagree about the
/// timeline, which is the one thing a per-take path choice must never cause.
unsafe fn submit_frame(
    session: &CFRetained<VTCompressionSession>,
    buffer: &CVPixelBuffer,
    frame_index: u64,
    fps: u32,
) -> Result<(), EncodeError> {
    // Frame 0 is a forced IDR so the stream always opens with a keyframe;
    // the per-second keyframe interval keeps them coming.
    unsafe { submit_frame_at(session, buffer, frame_index, fps, frame_index == 0) }
}

/// The zero-copy entry points' shared refusal: a buffer of the wrong shape
/// would be *interpreted* rather than rejected.
fn check_zero_copy_buffer(
    buffer: &CVPixelBuffer,
    width: u32,
    height: u32,
) -> Result<(), EncodeError> {
    if CVPixelBufferGetPixelFormatType(buffer) != kCVPixelFormatType_32BGRA {
        return Err(no_hardware(
            "zero-copy buffer is not 32BGRA; refusing to reinterpret its planes",
        ));
    }
    let (w, h) = (
        CVPixelBufferGetWidth(buffer),
        CVPixelBufferGetHeight(buffer),
    );
    if w != width as usize || h != height as usize {
        return Err(no_hardware(format!(
            "zero-copy buffer is {w}x{h}, session is {width}x{height}"
        )));
    }
    Ok(())
}

/// Submit one frame at PTS `pts_index / fps`, forcing an IDR when asked.
unsafe fn submit_frame_at(
    session: &CFRetained<VTCompressionSession>,
    buffer: &CVPixelBuffer,
    pts_index: u64,
    fps: u32,
    force_keyframe: bool,
) -> Result<(), EncodeError> {
    let frame_index = pts_index;
    unsafe {
        let pts = CMTime::new(pts_index as i64, fps as i32);
        let duration = CMTime::new(1, fps as i32);
        let frame_props = if force_keyframe {
            let key: &CFString = kVTEncodeFrameOptionKey_ForceKeyFrame;
            let value: &CFType = CFBoolean::new(true);
            Some(CFDictionary::<CFString, CFType>::from_slices(
                &[key],
                &[value],
            ))
        } else {
            None
        };
        let status = session.encode_frame(
            buffer,
            pts,
            duration,
            frame_props.as_ref().map(|dict| dict.as_opaque()),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if status != 0 {
            return Err(no_hardware(format!(
                "encoding frame {frame_index} failed (OSStatus {status})"
            )));
        }
        Ok(())
    }
}
