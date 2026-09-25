//! The zero-copy frame tap: one IOSurface seen by both the compositor and the
//! encoder, so a recorded frame is never copied to CPU memory.
//!
//! ZERO-COPY Phase 2, built on the chain Phase 1 proved end to end
//! (`docs/09-measurements.md`). This module exists in `nbe-decode` and not in
//! `nbe-engine` for one reason, and it is a policy reason rather than a taste
//! one: the workspace denies `unsafe_code` with exactly one exemption, this
//! crate, and CI refuses `allow(unsafe_code)` anywhere else under `src/`. Every
//! link below is Objective-C FFI.
//!
//! wgpu is an **optional** dependency behind `gpu-tap`, because `nbe-preflight`
//! also depends on this crate and never touches a GPU.
//!
//! ## The accessor, and why it is the one that matters
//!
//! The IOSurface-backed `MTLTexture` MUST be created on the `MTLDevice` that
//! wgpu selected — reachable via `wgpu_hal::metal::Device::raw_device()`. The
//! obvious alternative, `MTLCreateSystemDefaultDevice()`, is wrong on precisely
//! the machine this project targets: `docs/hardware-baseline.txt` is a dual-GPU
//! MacBook Pro (Intel UHD 630 and a Radeon Pro 555X), the engine asks wgpu for
//! `HighPerformance`, and the system default is not necessarily what it gets. A
//! texture created on the other device is not shareable with the one rendering
//! into it. [`probe`] fails rather than guessing when the accessor is gone.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString};
use objc2_core_video::{CVPixelBuffer, CVPixelBufferCreateWithIOSurface};
use objc2_io_surface::{
    kIOSurfaceBytesPerElement, kIOSurfaceHeight, kIOSurfacePixelFormat, kIOSurfaceWidth,
    IOSurfaceRef,
};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTextureDescriptor, MTLTextureUsage};

use crate::encode::EncodeError;

/// `'BGRA'`, the FourCC VideoToolbox takes as `kCVPixelFormatType_32BGRA` and
/// the format wgpu renders as `Bgra8Unorm`. One format agreed on both sides is
/// what makes the surface shareable at all.
const FOURCC_BGRA: i32 = 0x4247_5241;

/// Why a zero-copy tap could not be built. Every variant is loud: a tap that
/// cannot be built must say so, because the alternative is a silent fallback to
/// a path SPEC §0.1 assumption 24 only conditionally permits.
#[derive(Debug, thiserror::Error)]
pub enum ZeroCopyError {
    #[error("E_NO_ZEROCOPY: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Encode(#[from] EncodeError),
}

fn unavailable(msg: impl Into<String>) -> ZeroCopyError {
    ZeroCopyError::Unavailable(msg.into())
}

/// Allocate an IOSurface in the one format both sides agree on.
fn make_surface(width: u32, height: u32) -> Result<CFRetained<IOSurfaceRef>, ZeroCopyError> {
    if width == 0 || height == 0 {
        return Err(unavailable(format!(
            "implausible surface geometry {width}x{height}"
        )));
    }
    let keys: [&CFString; 4] = unsafe {
        [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfaceBytesPerElement,
            kIOSurfacePixelFormat,
        ]
    };
    let vals = [
        CFNumber::new_i32(width as i32),
        CFNumber::new_i32(height as i32),
        CFNumber::new_i32(4),
        CFNumber::new_i32(FOURCC_BGRA),
    ];
    let mut kp: Vec<*const c_void> = keys
        .iter()
        .map(|k| (*k) as *const CFString as *const _)
        .collect();
    let mut vp: Vec<*const c_void> = vals
        .iter()
        .map(|v| (&**v) as *const CFNumber as *const _)
        .collect();
    let dict = unsafe {
        CFDictionary::new(
            None,
            kp.as_mut_ptr(),
            vp.as_mut_ptr(),
            kp.len() as isize,
            std::ptr::null(),
            std::ptr::null(),
        )
    }
    .ok_or_else(|| unavailable("CFDictionary for IOSurface properties"))?;
    unsafe { IOSurfaceRef::new(&dict) }.ok_or_else(|| unavailable("IOSurfaceCreate returned NULL"))
}

/// A [`SharedSurface`]'s own reference on its `CVPixelBuffer`: the one it took
/// from `CVPixelBufferCreateWithIOSurface`. Any retain count above this is
/// another holder.
pub const PIXEL_BUFFER_BASELINE_RETAIN: usize = 1;

/// A frame buffer both the compositor and the encoder can see.
///
/// The wgpu texture and the `CVPixelBuffer` are two views of one allocation. A
/// frame is rendered into the texture and handed to VideoToolbox as the pixel
/// buffer, with nothing copied between.
#[derive(Debug)]
pub struct SharedSurface {
    surface: CFRetained<IOSurfaceRef>,
    pixel_buffer: CFRetained<CVPixelBuffer>,
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

// SAFETY: the two CF handles inside are `!Send + !Sync` only because objc2
// marks every `CFRetained<T>` that way by default — it cannot know which CF
// types are thread-safe, so it assumes none are. These two are:
//
//   * `IOSurfaceRef` exists to be shared. Its entire purpose is handing one
//     allocation between processes, between the CPU and a GPU, and between
//     threads; `IOSurfaceLock`/`Unlock` are its documented concurrency
//     primitives and CFRetain/CFRelease are atomic.
//   * `CVPixelBuffer` is a CF object over that same surface, created here by
//     `CVPixelBufferCreateWithIOSurface`. VideoToolbox is *documented* to be
//     handed pixel buffers from other threads — `VTCompressionSessionEncodeFrame`
//     is called from the record thread in this very design.
//
// `wgpu::Texture` is already `Send + Sync`.
//
// What the impls do NOT claim is that the PIXELS may be written concurrently.
// One writer at a time is a discipline, and `SurfacePool` is where it lives: a
// surface is handed out only while nobody else holds it, so the compositor
// never draws into a surface the encoder is reading. Remove the pool and these
// impls become a lie — which is why the pool is a precondition of the migration
// and not an optimisation of it.
unsafe impl Send for SharedSurface {}
unsafe impl Sync for SharedSurface {}

impl SharedSurface {
    /// The compositor's view. Render into this.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// The encoder's view. Hand this to VideoToolbox.
    pub fn pixel_buffer(&self) -> &CFRetained<CVPixelBuffer> {
        &self.pixel_buffer
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The IOSurface's id, which is what makes "same allocation" checkable from
    /// a test rather than asserted in a comment.
    pub fn surface_id(&self) -> u32 {
        self.surface.id()
    }

    /// The `CVPixelBuffer`'s Core Foundation retain count.
    ///
    /// This surface holds exactly one reference, so the baseline is
    /// [`PIXEL_BUFFER_BASELINE_RETAIN`]. Anything above it is another holder —
    /// in practice VideoToolbox, which retains the buffer when
    /// `VTCompressionSessionEncodeFrame` accepts it and releases it only once
    /// the frame has been encoded (the encode is asynchronous). This is how the
    /// pool's free rule can tell "no Rust owner" from "nobody reading".
    pub fn pixel_buffer_retain_count(&self) -> usize {
        let cf: &objc2_core_foundation::CFType = &self.pixel_buffer;
        cf.retain_count()
    }

    /// True when nothing but this surface references its `CVPixelBuffer` —
    /// in particular, when no encoder still holds the frame.
    pub fn encoder_released(&self) -> bool {
        self.pixel_buffer_retain_count() <= PIXEL_BUFFER_BASELINE_RETAIN
    }
}

/// Build a shared surface on the device **wgpu selected**.
///
/// Fails loudly on every link. A caller that gets an `Err` here has learned the
/// zero-copy path is unavailable on this machine and must select
/// `CpuReadback` — which is the fallback the selection table already describes.
pub fn probe(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<SharedSurface, ZeroCopyError> {
    let surface = make_surface(width, height)?;

    let desc = MTLTextureDescriptor::new();
    unsafe {
        desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        desc.setWidth(width as usize);
        desc.setHeight(height as usize);
        desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    }

    // The accessor this module's header argues for. If wgpu ever stops exposing
    // the selected device, this is where the chain must fail — NOT fall through
    // to MTLCreateSystemDefaultDevice, which would silently build the surface on
    // the wrong GPU of a dual-GPU machine.
    let mtl_texture = unsafe {
        device.as_hal::<wgpu::hal::api::Metal>().map(|hal| {
            hal.raw_device()
                .newTextureWithDescriptor_iosurface_plane(&desc, &surface, 0)
        })
    }
    .ok_or_else(|| unavailable("wgpu exposed no Metal hal device; refusing to guess a GPU"))?
    .ok_or_else(|| unavailable("newTextureWithDescriptor:iosurface:plane: returned nil"))?;

    let hal_texture = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            mtl_texture,
            wgpu::TextureFormat::Bgra8Unorm,
            objc2_metal::MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
            None,
        )
    };
    let texture = unsafe {
        device.create_texture_from_hal::<wgpu::hal::api::Metal>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("record-tap-iosurface"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                // COPY_SRC and COPY_DST are NOT optional here, and their
                // absence was found by the step-4 retarget test rather than
                // reasoned about: `readback_view` copies the View to a buffer,
                // so without COPY_SRC the first `readback_view` during a
                // zero-copy take aborts with
                //   "Usage flags TextureUsages(TEXTURE_BINDING |
                //    RENDER_ATTACHMENT) ... do not contain required usage
                //    flags TextureUsages(COPY_DST)"
                // and every golden-frame suite that inspects the View goes with
                // it. The memo's Q2 GO rests on exactly that readback still
                // working across the retarget.
                //
                // Free on the Metal side: `MTLTextureUsage` has no blit bit —
                // copies are always permitted — so this widens what wgpu will
                // validate, not what the texture can do.
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::TextureUses::COLOR_TARGET,
        )
    };

    let mut raw: *mut CVPixelBuffer = std::ptr::null_mut();
    let rc = unsafe {
        CVPixelBufferCreateWithIOSurface(
            None,
            &surface,
            None,
            NonNull::new(&mut raw).expect("a stack out-pointer is non-null"),
        )
    };
    if rc != 0 {
        return Err(unavailable(format!(
            "CVPixelBufferCreateWithIOSurface failed (CVReturn {rc})"
        )));
    }
    let pixel_buffer = unsafe {
        CFRetained::from_raw(
            NonNull::new(raw)
                .ok_or_else(|| unavailable("CVPixelBufferCreateWithIOSurface gave null"))?,
        )
    };

    Ok(SharedSurface {
        surface,
        pixel_buffer,
        texture,
        width,
        height,
    })
}

/// Is the zero-copy chain usable on this device, at this geometry?
///
/// The honest probe: it builds the whole thing and throws it away. A cheaper
/// check would be a guess about what the stack supports, and the selection table
/// is not worth more than the probe behind it.
pub fn is_available(device: &wgpu::Device, width: u32, height: u32) -> bool {
    probe(device, width, height).is_ok()
}

// ---------------------------------------------------------------------------
// The surface pool
// ---------------------------------------------------------------------------

/// N shared surfaces, so the compositor can draw frame N+1 while the encoder
/// still reads frame N.
///
/// **This is the design, not a refinement of it** (`docs/zero-copy-p3-design.md`,
/// Q3). The record path's existing backpressure discipline — *"a full channel
/// sheds (never blocks) and the loop counts the skip"* — does not transfer to
/// one shared surface. `RecordMsg::Frame { rgba }` carries an owned copy per
/// frame, so shedding is free: drop the `Vec` and the compositor's next frame
/// has nothing to do with it. A shared surface is ONE MUTABLE ALLOCATION. If
/// the encoder is still reading frame N when the compositor starts frame N+1,
/// "shed" is not available — the pixels are already overwritten. Shedding a
/// surface you have drawn into is not a skip, it is a corrupted frame.
///
/// So the question moves earlier: **is a free surface available?**, asked
/// BEFORE the draw. No free surface → skip before rendering, which keeps the
/// discipline's actual promise (record degrades, the View never waits) at the
/// only point where it can still be kept.
///
/// ## Why `Arc::strong_count` is the free list
///
/// A surface is free exactly when nobody outside the pool holds it. The pool is
/// the only place clones are handed out, and [`Self::acquire`] is called from
/// one place — the render loop — so the count can only fall asynchronously, as
/// the record thread drops what it finished with. A count of 1 therefore means
/// *definitely free*; a higher count may be a surface freed a microsecond ago
/// and read as busy. The error is one-sided by construction: it can cost a
/// skipped record frame, and can never hand out a surface still in flight.
#[derive(Debug)]
pub struct SurfacePool {
    surfaces: Vec<std::sync::Arc<SharedSurface>>,
    width: u32,
    height: u32,
}

impl SurfacePool {
    /// Build `count` surfaces at this geometry, on the device wgpu selected.
    ///
    /// Every surface comes from the same [`probe`], so a pool that builds is a
    /// pool whose every member is the chain Phase 1 proved. Fails loudly on the
    /// first surface that will not build: a partial pool would silently lower
    /// the frame rate the tap can sustain.
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        count: usize,
    ) -> Result<Self, ZeroCopyError> {
        if count == 0 {
            return Err(unavailable("a surface pool of zero surfaces is not a pool"));
        }
        let mut surfaces = Vec::with_capacity(count);
        for i in 0..count {
            let s = probe(device, width, height).map_err(|e| {
                unavailable(format!("surface {i} of {count} could not be built: {e}"))
            })?;
            surfaces.push(std::sync::Arc::new(s));
        }
        Ok(Self {
            surfaces,
            width,
            height,
        })
    }

    pub fn len(&self) -> usize {
        self.surfaces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// How many surfaces nobody else is holding right now.
    pub fn free(&self) -> usize {
        self.surfaces.iter().filter(|s| Self::is_free(s)).count()
    }

    /// The free rule: no Rust holder AND no encoder holder.
    ///
    /// **`Arc::strong_count == 1` alone was not enough, and that shipped in
    /// merged code** (ZERO-COPY Phase 3b's record path; PR #30 doubled the
    /// exposure with a second encoder). `VTCompressionSessionEncodeFrame`
    /// *retains* the `CVPixelBuffer` it is handed and encodes asynchronously:
    /// measured on the reference machine, the buffer's retain count reads 1
    /// before `encode_pixel_buffer`, 2 the moment it returns, and falls back to
    /// 1 between 1.6 ms and 17.5 ms later. The record thread drops its `Arc`
    /// as soon as `encode_pixel_buffer` returns, so a count-only rule marked
    /// the surface free while VideoToolbox was still reading its pixels — and
    /// the compositor could draw the next frame into it. ~~At 30 fps that
    /// window is usually shorter than a frame; at 60 fps (16.7 ms) the measured
    /// worst case already exceeds it.~~ Precisely: a frame is overwritten while
    /// VideoToolbox reads it exactly when the gap between the consumer's drop
    /// and the loop's next acquire is shorter than VideoToolbox's hold
    /// (1.6–3.7 ms steady, 17.5 ms on the first frame). A keeping-pace thread
    /// at 30 fps leaves ~31 ms; the window opens when the thread runs late, and
    /// at 60 fps the ~15 ms steady gap does not cover the first-frame hold. The
    /// result is a torn frame in the file. No shipped recording has been
    /// audited (`docs/09-measurements.md`, Prompt 10).
    ///
    /// The retain chain is VideoToolbox's own statement of when it is done,
    /// so the rule reads it: a surface is free only when its `CVPixelBuffer`
    /// is back at [`PIXEL_BUFFER_BASELINE_RETAIN`]. Still one-sided — a
    /// release that lands a microsecond after the check costs a skip, never a
    /// corrupted frame.
    ///
    /// **Why the fence, and why it is proof rather than hope** (construction
    /// argument, written because no test on the normative x86 machine can
    /// flip on its removal — see below):
    ///
    /// * A consumer thread C calls `VTCompressionSessionEncodeFrame`, which
    ///   retains the buffer (R) before it returns: the header lets the client
    ///   release its own reference the moment the call returns, so the
    ///   session's retain must already be in place. C then drops its `Arc`:
    ///   `strong.fetch_sub(1, Release)` (D). R is sequenced before D.
    /// * This thread reads `Arc::strong_count` — a `Relaxed` load (L) in std —
    ///   and sees 1, the value D wrote (with two consumers, D is the later of
    ///   two RMW decrements, which is in the first one's release sequence).
    /// * `fence(Acquire)` (F) after L: by the fence rule, D synchronizes-with
    ///   F because L, sequenced before F, read the value D wrote. So R
    ///   happens-before the retain-count read below (G), and G returns R's
    ///   increment or a later value — never the pre-encode baseline while
    ///   VideoToolbox still holds the buffer. A release that lands after G
    ///   costs a skip, never a corrupted frame.
    /// * Without F, L's `Relaxed` load orders nothing: on a weakly ordered CPU
    ///   (arm64 — Apple Silicon, which §0.1 assumption 2 supports though the
    ///   reference target is the Intel Mac, and which the macos-14 CI runner
    ///   is; ~~"the spec's primary target"~~ was v0.3's wording, corrected in
    ///   v0.4) G may observe the
    ///   retain count's older value after L observed 1, and hand out a
    ///   surface VideoToolbox is reading. On x86-64 loads are not reordered
    ///   with older loads, so the hazard cannot manifest there; LLVM lowers
    ///   `fence acquire` to no instruction on x86-64 and to `dmb ishld` on
    ///   arm64, which is why removing it cannot flip a test on this machine.
    ///   (PR #30's first version of this rule had no fence; found by its
    ///   own-author pass.)
    fn is_free(s: &std::sync::Arc<SharedSurface>) -> bool {
        if std::sync::Arc::strong_count(s) != 1 {
            return false;
        }
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        s.encoder_released()
    }

    /// Take a free surface, or `None` when every one is still in flight.
    ///
    /// `None` is the signal to skip this record frame **before drawing it**.
    /// Never hands out a surface another holder still has — including an
    /// encoder that has not finished reading it (see [`Self::is_free`]).
    pub fn acquire(&self) -> Option<std::sync::Arc<SharedSurface>> {
        self.surfaces
            .iter()
            .find(|s| Self::is_free(s))
            .map(std::sync::Arc::clone)
    }

    /// Every surface's IOSurface id, for tests that need to check "same
    /// allocation" rather than assert it in a comment.
    pub fn surface_ids(&self) -> Vec<u32> {
        self.surfaces.iter().map(|s| s.surface_id()).collect()
    }
}
