//! Camera capture (AVFoundation), for the engine's live camera layer
//! (sprint PR C). `unsafe` lives here because the engine crate denies it.
//!
//! An `AVCaptureSession` with one video device input and one
//! `AVCaptureVideoDataOutput` asked for 32BGRA at 1280x720. Each frame
//! arrives on a serial dispatch queue; the delegate copies it out of the
//! pixel buffer into a tightly packed `Vec` and hands it to the caller's
//! callback, then returns the buffer to the device at once (holding sample
//! buffers starves the capture pool). Late frames are discarded by the
//! output, not queued: a live picture wants the newest frame, not every one.

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput, AVCaptureOutput, AVCaptureSession,
    AVCaptureSessionPreset1280x720, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{
    kCVPixelBufferPixelFormatTypeKey, kCVReturnSuccess, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetDataSize, CVPixelBufferGetHeight,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use std::sync::atomic::{AtomicU64, Ordering};

/// `kCVPixelFormatType_32BGRA`.
const PIXEL_FORMAT_32BGRA: u32 = 0x42475241;

/// Called once per captured frame: width, height, BGRA bytes (`width * 4`
/// a row), and the session's own frame count from 1.
pub type FrameCallback = Box<dyn Fn(u32, u32, Vec<u8>, u64) + Send + Sync>;

pub struct DelegateIvars {
    on_frame: FrameCallback,
    seq: AtomicU64,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the delegate
    // holds no Objective-C state of its own.
    #[unsafe(super(NSObject))]
    #[name = "NBECameraFrameDelegate"]
    #[ivars = DelegateIvars]
    struct FrameDelegate;

    unsafe impl NSObjectProtocol for FrameDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for FrameDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn capture_output(
            &self,
            _output: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            let ivars = self.ivars();
            if let Some((w, h, bgra)) = copy_bgra(sample) {
                let seq = ivars.seq.fetch_add(1, Ordering::Relaxed) + 1;
                (ivars.on_frame)(w, h, bgra, seq);
            }
        }
    }
);

impl FrameDelegate {
    fn new(on_frame: FrameCallback) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars {
            on_frame,
            seq: AtomicU64::new(0),
        });
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Copy a 32BGRA sample buffer out, tightly packed. `None` for anything
/// else: a buffer of another format, one that will not lock, or one whose
/// stated size cannot hold its rows.
fn copy_bgra(sample: &CMSampleBuffer) -> Option<(u32, u32, Vec<u8>)> {
    // SAFETY: every read of the pixel buffer's memory below happens between
    // a successful lock and the unlock, and only after checking that the
    // buffer's data size covers `stride * (height - 1) + width * 4` bytes.
    unsafe {
        let image = sample.image_buffer()?;
        if CVPixelBufferGetPixelFormatType(&image) != PIXEL_FORMAT_32BGRA {
            return None;
        }
        let width = CVPixelBufferGetWidth(&image);
        let height = CVPixelBufferGetHeight(&image);
        if width == 0 || height == 0 || width > 8192 || height > 8192 {
            return None;
        }
        if CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly)
            != kCVReturnSuccess
        {
            return None;
        }
        let base = CVPixelBufferGetBaseAddress(&image) as *const u8;
        let stride = CVPixelBufferGetBytesPerRow(&image);
        let size = CVPixelBufferGetDataSize(&image);
        let row = width * 4;
        let fits = stride
            .checked_mul(height - 1)
            .and_then(|o| o.checked_add(row))
            .is_some_and(|need| need <= size);
        let out = if base.is_null() || stride < row || !fits {
            None
        } else {
            let mut bgra = vec![0u8; row * height];
            for y in 0..height {
                std::ptr::copy_nonoverlapping(
                    base.add(y * stride),
                    bgra.as_mut_ptr().add(y * row),
                    row,
                );
            }
            Some((width as u32, height as u32, bgra))
        };
        CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly);
        out
    }
}

/// A running capture session. Dropping it stops the session.
pub struct CameraSession {
    session: Retained<AVCaptureSession>,
    // Kept alive for the session's lifetime: the output holds the delegate
    // weakly.
    _delegate: Retained<FrameDelegate>,
    _queue: dispatch2::DispatchRetained<dispatch2::DispatchQueue>,
}

// SAFETY: AVCaptureSession is documented thread-safe for start/stop; the
// handle is only used to stop the session on drop.
unsafe impl Send for CameraSession {}

impl Drop for CameraSession {
    fn drop(&mut self) {
        // SAFETY: stopRunning may be called from any thread.
        unsafe { self.session.stopRunning() };
    }
}

/// Open a camera: `"default"` for the system's default video device, else a
/// device unique id (`AVCaptureDevice.uniqueID`). Starts it and returns once
/// the session is running; frames then arrive on `on_frame`.
///
/// Camera access is the calling process's (on macOS, the terminal's or the
/// app's) privacy grant; a process without one gets no frames, which the
/// engine treats as a lost camera.
pub fn open_camera(device: &str, on_frame: FrameCallback) -> Result<CameraSession, String> {
    // SAFETY: AVFoundation calls with valid, retained arguments; the media
    // type constant is a static NSString.
    unsafe {
        let media = AVMediaTypeVideo.ok_or("AVMediaTypeVideo unavailable")?;
        let dev = if device == "default" {
            AVCaptureDevice::defaultDeviceWithMediaType(media)
        } else {
            AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(device))
        }
        .ok_or_else(|| format!("no camera device {device:?}"))?;
        let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&dev).map_err(|e| {
            format!(
                "camera {device:?} would not open: {}",
                e.localizedDescription()
            )
        })?;

        let session = AVCaptureSession::new();
        session.beginConfiguration();
        if session.canSetSessionPreset(AVCaptureSessionPreset1280x720) {
            session.setSessionPreset(AVCaptureSessionPreset1280x720);
        }
        if !session.canAddInput(&input) {
            return Err(format!("camera {device:?}: the session refused its input"));
        }
        session.addInput(&input);

        let output = AVCaptureVideoDataOutput::new();
        let key: &NSString = &*(kCVPixelBufferPixelFormatTypeKey as *const _ as *const NSString);
        let value = NSNumber::new_u32(PIXEL_FORMAT_32BGRA);
        let settings = NSDictionary::from_slices::<NSString>(&[key], &[&*value as &_]);
        output.setVideoSettings(Some(&settings));
        output.setAlwaysDiscardsLateVideoFrames(true);
        let delegate = FrameDelegate::new(on_frame);
        let queue = dispatch2::DispatchQueue::new("nbe.camera.frames", None);
        output.setSampleBufferDelegate_queue(
            Some(ProtocolObject::from_ref(&*delegate)),
            Some(&queue),
        );
        if !session.canAddOutput(&output) {
            return Err(format!("camera {device:?}: the session refused its output"));
        }
        session.addOutput(&output);
        session.commitConfiguration();
        session.startRunning();
        if !session.isRunning() {
            return Err(format!("camera {device:?}: the session did not start"));
        }
        Ok(CameraSession {
            session,
            _delegate: delegate,
            _queue: queue,
        })
    }
}
