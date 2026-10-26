//! The device sink: program audio to an output device through cpal, macOS
//! only (the device-audio design record, D1, D2 and D7).
//!
//! The audio thread pushes rendered blocks into a lock-free ring
//! ([`crate::audio_ring`]); cpal's callback copies them out and publishes one
//! presentation mark per callback ([`crate::audio_clock::DeviceMarks`]). The
//! callback touches nothing else: no graph, no lock, no allocation, no I/O
//! (SPEC §8.9). An empty ring plays silence and is counted.
//!
//! **The rate guard (D1 a).** At open the sink reads the device's nominal
//! rate. At 48 kHz it opens and changes nothing; otherwise it switches the
//! device to 48 kHz itself, logs both rates, and restores the prior rate when
//! it closes (drop, which the engine reaches on an orderly shutdown). A device
//! that cannot run at 48 kHz is refused. **The crash-path limit, named:** a
//! crash, a `SIGKILL`, or an aborting panic runs no restore, and the device
//! stays at 48 kHz until someone changes it; the open's log line names the
//! prior rate, so an operator can.
//!
//! **The presentation time (D1 b).** cpal's `playback` instant includes the
//! buffer, the device's latency and its safety offset, and leaves out the
//! stream's latency (design record §2.2). The sink reads that once at open,
//! through `nbe_decode::hal`, and adds it to every mark.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use nbe_decode::hal;

use crate::audio::{CHANNELS, SAMPLE_RATE};
use crate::audio_clock::{copy_out, presented_ns, DeviceMarks};
use crate::audio_driver::{rate_plan, AudioSink, DeviceInfo, DeviceIo, RatePlan, RING_BLOCKS};
use crate::audio_ring;

/// Which device to open. The engine opens the default device (D8: selection
/// by name is out of this era); `NameContains` is the normative machine's
/// test seam for its loopback cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceChoice {
    Default,
    NameContains(String),
}

/// Why a device did not open. The engine falls back to silence and reports
/// the reason on the tick's device line.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("no output device{0}")]
    NoDevice(String),
    #[error("the device {name} cannot run at 48 kHz (nominal {nominal_hz} Hz): refused (D1 a)")]
    Refused { name: String, nominal_hz: f64 },
    #[error("the device {name} did not reach 48 kHz within {waited:?} (it reads {reads_hz} Hz)")]
    RateNotApplied {
        name: String,
        waited: Duration,
        reads_hz: f64,
    },
    #[error(transparent)]
    Hal(#[from] hal::HalError),
    #[error("cpal: {0}")]
    Cpal(String),
}

/// How long the rate guard waits for the HAL to apply 48 kHz.
const RATE_SWITCH_WAIT: Duration = Duration::from_millis(1000);

/// An open output device.
pub struct DeviceSink {
    io: DeviceIo,
    stream: Option<cpal::Stream>,
    hal_id: hal::DeviceId,
    block_frames: usize,
}

impl DeviceSink {
    /// Open `choice` at 48 kHz, stereo float32, and start it.
    pub fn open(choice: &DeviceChoice, block_frames: usize) -> Result<Self, OpenError> {
        let host = cpal::default_host();
        let (device, hal_id) = match choice {
            DeviceChoice::Default => (
                host.default_output_device()
                    .ok_or_else(|| OpenError::NoDevice(String::new()))?,
                hal::default_output_device()?,
            ),
            DeviceChoice::NameContains(needle) => {
                let device = host
                    .output_devices()
                    .map_err(|e| OpenError::Cpal(e.to_string()))?
                    .find(|d| {
                        d.description()
                            .map(|x| x.name().contains(needle.as_str()))
                            .unwrap_or(false)
                    })
                    .ok_or_else(|| OpenError::NoDevice(format!(" named like {needle:?}")))?;
                let id = hal::device_named(needle)?
                    .ok_or_else(|| OpenError::NoDevice(format!(" named like {needle:?} (HAL)")))?;
                (device, id)
            }
        };
        let name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| format!("<device {hal_id}>"));

        // The rate guard (D1 a).
        let nominal = hal::nominal_rate(hal_id)?;
        let supports_48k = device
            .supported_output_configs()
            .map_err(|e| OpenError::Cpal(e.to_string()))?
            .any(|c| {
                c.channels() as usize >= CHANNELS
                    && c.sample_format() == cpal::SampleFormat::F32
                    && c.min_sample_rate() <= SAMPLE_RATE
                    && SAMPLE_RATE <= c.max_sample_rate()
            });
        let prior_rate_hz = match rate_plan(nominal, supports_48k) {
            RatePlan::Keep => None,
            RatePlan::Refuse => {
                return Err(OpenError::Refused {
                    name,
                    nominal_hz: nominal,
                })
            }
            RatePlan::Switch { prior_hz } => {
                hal::set_nominal_rate(hal_id, SAMPLE_RATE as f64)?;
                let started = Instant::now();
                loop {
                    let reads = hal::nominal_rate(hal_id)?;
                    if reads == SAMPLE_RATE as f64 {
                        break;
                    }
                    if started.elapsed() > RATE_SWITCH_WAIT {
                        // Put back what was there before giving up.
                        let _ = hal::set_nominal_rate(hal_id, prior_hz);
                        return Err(OpenError::RateNotApplied {
                            name,
                            waited: RATE_SWITCH_WAIT,
                            reads_hz: reads,
                        });
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                tracing::info!(
                    device = %name,
                    prior_hz,
                    now_hz = SAMPLE_RATE,
                    "audio device switched to 48 kHz by the engine; the prior rate is restored at close (D1 a)"
                );
                Some(prior_hz)
            }
        };

        // The presentation offset cpal leaves out (D1 b), read at 48 kHz.
        let latency = hal::output_latency(hal_id)?;
        let stream_frames = latency.stream;

        let (producer, mut consumer) = audio_ring::ring(block_frames * RING_BLOCKS);
        let marks = DeviceMarks::new();
        let cb_marks = marks.clone();
        let data = move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
            // Real-time: the arrival instant, cpal's offset to playback, and
            // `copy_out` (atomics and copies only, SPEC §8.9).
            let stall = cb_marks.stall_next_ms.swap(0, Ordering::Relaxed);
            if stall > 0 {
                // The forced-stall test seam (never set in production).
                std::thread::sleep(Duration::from_millis(stall));
            }
            let arrived = Instant::now();
            let ts = info.timestamp();
            let ahead = ts
                .playback
                .saturating_duration_since(ts.callback)
                .as_nanos() as u64;
            let presented = presented_ns(cb_marks.ns_of(arrived), ahead, stream_frames);
            copy_out(&mut consumer, &cb_marks, out, presented);
        };
        let err_marks = marks.clone();
        let error = move |e: cpal::Error| match e.kind() {
            cpal::ErrorKind::Xrun => {
                err_marks.xruns.fetch_add(1, Ordering::Relaxed);
            }
            cpal::ErrorKind::DeviceNotAvailable
            | cpal::ErrorKind::StreamInvalidated
            | cpal::ErrorKind::DeviceChanged => {
                err_marks.lost.store(true, Ordering::Relaxed);
            }
            _ => {}
        };
        let config = cpal::StreamConfig {
            channels: CHANNELS as cpal::ChannelCount,
            sample_rate: SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Default,
        };
        let opened = device
            .build_output_stream::<f32, _, _>(config, data, error, None)
            .map_err(|e| OpenError::Cpal(e.to_string()))
            .and_then(|stream| {
                stream
                    .play()
                    .map(|()| stream)
                    .map_err(|e| OpenError::Cpal(e.to_string()))
            });
        let stream = match opened {
            Ok(s) => s,
            Err(e) => {
                if let Some(prior) = prior_rate_hz {
                    let _ = hal::set_nominal_rate(hal_id, prior);
                }
                return Err(e);
            }
        };
        let latency_frames = latency.device + latency.safety_offset + latency.stream;
        tracing::info!(
            device = %name,
            rate_hz = SAMPLE_RATE,
            latency_frames,
            device_latency = latency.device,
            safety_offset = latency.safety_offset,
            stream_latency = latency.stream,
            "audio device open: program audio on air"
        );
        Ok(Self {
            io: DeviceIo {
                producer,
                marks,
                info: DeviceInfo {
                    name,
                    rate_hz: SAMPLE_RATE,
                    prior_rate_hz,
                    latency_frames,
                },
            },
            stream: Some(stream),
            hal_id,
            block_frames,
        })
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.io.info
    }

    pub fn marks(&self) -> Arc<DeviceMarks> {
        self.io.marks.clone()
    }
}

impl Drop for DeviceSink {
    /// Close: stop the stream, then restore the device's prior rate (D1 a).
    fn drop(&mut self) {
        drop(self.stream.take());
        if let Some(prior) = self.io.info.prior_rate_hz {
            match hal::set_nominal_rate(self.hal_id, prior) {
                Ok(()) => tracing::info!(
                    device = %self.io.info.name,
                    restored_hz = prior,
                    "audio device closed; its prior rate restored (D1 a)"
                ),
                Err(e) => tracing::error!(
                    device = %self.io.info.name,
                    prior_hz = prior,
                    error = %e,
                    "audio device closed; its prior rate could NOT be restored"
                ),
            }
        }
    }
}

impl AudioSink for DeviceSink {
    fn write(&mut self, block: &[f32]) -> bool {
        self.io.producer.push(block) == block.len()
    }

    fn block_frames(&self) -> usize {
        self.block_frames
    }

    fn device(&mut self) -> Option<&mut DeviceIo> {
        Some(&mut self.io)
    }
}
