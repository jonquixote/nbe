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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
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
