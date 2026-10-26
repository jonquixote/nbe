//! CoreAudio HAL property reads for the engine's device sink (device-audio
//! design D1).
//!
//! cpal opens and drives the output stream, and its API is safe. Two things
//! it does not expose, both ruled required on 2026-10-21:
//!
//! - **(a) the device's nominal rate**, so the sink switches the device to
//!   48 kHz as its own explicit act and restores the prior rate at close.
//!   cpal switches it as a side effect and never restores it (design record
//!   §2.2);
//! - **(b) the output stream's latency**, which cpal's presentation estimate
//!   leaves out: 275 frames, 5.7 ms, on the normative machine's speakers
//!   (§2.2).
//!
//! Both are `AudioObjectGetPropertyData` / `AudioObjectSetPropertyData` calls,
//! `unsafe` by construction, so they live in this crate under its single
//! exception. **The contract each call keeps:** a valid
//! `AudioObjectPropertyAddress`, a data size equal to the property's type, and
//! a pointer valid for that size; the `OSStatus` is checked and returned,
//! never assumed. Everything this module hands out is plain data.

use std::ffi::c_void;
use std::ptr::{null, NonNull};

use objc2_core_audio::{
    kAudioDevicePropertyLatency, kAudioDevicePropertyNominalSampleRate,
    kAudioDevicePropertySafetyOffset, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject, kAudioStreamPropertyLatency,
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectPropertyAddress,
    AudioObjectSetPropertyData,
};
use objc2_core_foundation::{CFRetained, CFString};

/// A CoreAudio object id (`AudioObjectID`).
pub type DeviceId = u32;

/// A HAL call that did not return `noErr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("CoreAudio {call} failed: OSStatus {status}")]
pub struct HalError {
    pub call: &'static str,
    pub status: i32,
}

/// An output device's latency, in frames at its nominal rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputLatency {
    /// `kAudioDevicePropertyLatency`, output scope.
    pub device: u32,
    /// `kAudioDevicePropertySafetyOffset`, output scope.
    pub safety_offset: u32,
    /// `kAudioStreamPropertyLatency` of the device's first output stream:
    /// the part cpal's estimate omits.
    pub stream: u32,
}

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Read a fixed-size property of type `T`.
fn get<T: Copy + Default>(
    id: DeviceId,
    selector: u32,
    scope: u32,
    call: &'static str,
) -> Result<T, HalError> {
    let addr = address(selector, scope);
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `addr` is a valid property address for the duration of the
    // call; `size` is exactly `size_of::<T>()`, and `value` is a live `T` of
    // that size, so the HAL writes at most `size` bytes into owned memory.
    let status = unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&addr),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast::<c_void>(),
        )
    };
    if status != 0 {
        return Err(HalError { call, status });
    }
    Ok(value)
}

/// The system's default output device.
pub fn default_output_device() -> Result<DeviceId, HalError> {
    get::<u32>(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
        "default output device",
    )
}

/// The device's nominal sample rate, in Hz.
pub fn nominal_rate(id: DeviceId) -> Result<f64, HalError> {
    get::<f64>(
        id,
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
        "nominal rate (get)",
    )
}

/// Set the device's nominal sample rate. The HAL applies it asynchronously:
/// read [`nominal_rate`] until it reports the new value.
pub fn set_nominal_rate(id: DeviceId, hz: f64) -> Result<(), HalError> {
    let addr = address(
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
    );
    // SAFETY: `addr` is a valid property address; the data size is
    // `size_of::<f64>()`, the property's type (`Float64`), and `hz` is a live
    // `f64` the HAL only reads.
    let status = unsafe {
        AudioObjectSetPropertyData(
            id,
            NonNull::from(&addr),
            0,
            null(),
            std::mem::size_of::<f64>() as u32,
            NonNull::from(&hz).cast::<c_void>(),
        )
    };
    if status != 0 {
        return Err(HalError {
            call: "nominal rate (set)",
            status,
        });
    }
    Ok(())
}

/// The device's output streams.
fn output_streams(id: DeviceId) -> Result<Vec<u32>, HalError> {
    get_u32_list(
        id,
        kAudioDevicePropertyStreams,
        kAudioObjectPropertyScopeOutput,
    )
}

/// Every audio device the HAL knows, inputs and outputs alike.
pub fn devices() -> Result<Vec<DeviceId>, HalError> {
    get_u32_list(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    )
}

/// The first device whose name contains `needle`. A test seam for the
/// normative machine's loopback cases (VB-Cable); the engine itself opens
/// only the default device this era (design D8: selection by name is out).
pub fn device_named(needle: &str) -> Result<Option<DeviceId>, HalError> {
    for id in devices()? {
        if device_name(id)?.contains(needle) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// The device's name (`kAudioObjectPropertyName`).
pub fn device_name(id: DeviceId) -> Result<String, HalError> {
    let addr = address(kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal);
    let mut name: *const CFString = null();
    let mut size = std::mem::size_of::<*const CFString>() as u32;
    // SAFETY: the property's type is a `CFStringRef`, and `name` is a live
    // pointer-sized slot of exactly `size` bytes for the HAL to write it into.
    let status = unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&addr),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut name).cast::<c_void>(),
        )
    };
    if status != 0 {
        return Err(HalError {
            call: "device name",
            status,
        });
    }
    let Some(name) = NonNull::new(name as *mut CFString) else {
        return Ok(String::new());
    };
    // SAFETY: the HAL returns the name under the Create rule (+1), so taking
    // ownership releases it exactly once, when `owned` drops.
    let owned: CFRetained<CFString> = unsafe { CFRetained::from_raw(name) };
    Ok(owned.to_string())
}

/// A property that is a list of `u32` object ids.
fn get_u32_list(id: DeviceId, selector: u32, scope: u32) -> Result<Vec<u32>, HalError> {
    let addr = address(selector, scope);
    let mut size = 0u32;
    // SAFETY: `addr` is valid, and the HAL writes one `u32` into `size`.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            id,
            NonNull::from(&addr),
            0,
            null(),
            NonNull::from(&mut size),
        )
    };
    if status != 0 {
        return Err(HalError {
            call: "object list (size)",
            status,
        });
    }
    let mut ids = vec![0u32; size as usize / std::mem::size_of::<u32>()];
    if ids.is_empty() {
        return Ok(ids);
    }
    let mut size = (ids.len() * std::mem::size_of::<u32>()) as u32;
    // SAFETY: `ids` owns exactly `size` bytes of `u32`s, the property's
    // element type (an `AudioObjectID`), and `size` says so.
    let status = unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&addr),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::new(ids.as_mut_ptr())
                .expect("a non-empty Vec's pointer is non-null")
                .cast::<c_void>(),
        )
    };
    if status != 0 {
        return Err(HalError {
            call: "object list",
            status,
        });
    }
    ids.truncate(size as usize / std::mem::size_of::<u32>());
    Ok(ids)
}

/// The device's output latency: the device's own, its safety offset, and its
/// first output stream's.
pub fn output_latency(id: DeviceId) -> Result<OutputLatency, HalError> {
    let device = get::<u32>(
        id,
        kAudioDevicePropertyLatency,
        kAudioObjectPropertyScopeOutput,
        "device latency",
    )?;
    let safety_offset = get::<u32>(
        id,
        kAudioDevicePropertySafetyOffset,
        kAudioObjectPropertyScopeOutput,
        "safety offset",
    )?;
    let stream = match output_streams(id)?.first() {
        Some(&s) => get::<u32>(
            s,
            kAudioStreamPropertyLatency,
            kAudioObjectPropertyScopeGlobal,
            "stream latency",
        )?,
        None => 0,
    };
    Ok(OutputLatency {
        device,
        safety_offset,
        stream,
    })
}
