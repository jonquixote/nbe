//! Live inputs: a camera's frames into a layer the render loop never waits on
//! (SPEC §7.4 `camera` elements, §10.3 fallback trigger 3 "camera device
//! loss"; the sprint ruling of 2026-11-12, PR C).
//!
//! Three stages, each on its own thread, joined by two hand-offs that only
//! ever hold the *newest* thing:
//!
//! 1. **Capture** (the camera's own callback thread, or [`SyntheticCamera`]):
//!    each frame replaces the one in the source's [`FrameMailbox`]. A frame
//!    nobody uploaded before the next arrived is overwritten and counted —
//!    a camera that runs faster than the uploader loses frames, never time.
//! 2. **Upload** ([`LiveUploader`], one thread per source, started by the
//!    render loop with clones of its device and queue): takes the newest
//!    frame and writes it into one of three textures, then publishes which.
//!    The `write_texture` staging copy is this thread's, not the render
//!    thread's.
//! 3. **Draw** (the render loop): reads the published texture index — an
//!    atomic load — and samples it. No lock is waited on and no copy is made
//!    on the frame path. A source with nothing published yet draws nothing.
//!
//! **Loss.** A camera layer on the View whose newest frame is older than
//! [`LIVE_LOSS_MS`] engages the fallback slate (`FallbackSource::LiveLoss`);
//! fresh frames for [`LIVE_RECOVER_MS`] release it. The slate is a whole-View
//! cut, as the other fallback triggers are.
//!
//! **Which device.** The manifest names a `cameraId`; this machine maps it to
//! a device with `NBE_CAMERAS` (`main=default,wide=<device unique id>`,
//! or `main=synthetic` for the test source). An unmapped id opens the
//! system's default camera.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A camera layer whose newest frame is older than this puts the slate up.
pub const LIVE_LOSS_MS: u64 = 500;
/// Fresh frames for this long take it down again.
pub const LIVE_RECOVER_MS: u64 = 250;

/// One captured frame, BGRA8, tightly packed (`width * 4` bytes a row).
#[derive(Debug)]
pub struct LiveFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// The source's own count of frames it has produced, from 1.
    pub seq: u64,
}

/// The newest frame from one source, and when it came.
#[derive(Debug)]
pub struct FrameMailbox {
    slot: Mutex<Option<LiveFrame>>,
    ready: Condvar,
    epoch: Instant,
    /// Milliseconds after `epoch` the newest frame arrived; `u64::MAX` before any.
    last_at_ms: AtomicU64,
    /// Frames published by the capture side.
    pub frames_in: AtomicU64,
    /// Frames replaced before the uploader took them.
    pub frames_overwritten: AtomicU64,
}

impl Default for FrameMailbox {
    fn default() -> Self {
        Self {
            slot: Mutex::new(None),
            ready: Condvar::new(),
            epoch: Instant::now(),
            last_at_ms: AtomicU64::new(u64::MAX),
            frames_in: AtomicU64::new(0),
            frames_overwritten: AtomicU64::new(0),
        }
    }
}

impl FrameMailbox {
    /// The capture side: replace the newest frame.
    pub fn publish(&self, frame: LiveFrame) {
        let mut slot = self.slot.lock().unwrap();
        if slot.is_some() {
            self.frames_overwritten.fetch_add(1, Ordering::Relaxed);
        }
        *slot = Some(frame);
        drop(slot);
        self.frames_in.fetch_add(1, Ordering::Relaxed);
        self.last_at_ms
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::SeqCst);
        self.ready.notify_one();
    }

    /// The uploader: take the newest frame, waiting up to `timeout` for one.
    pub fn take(&self, timeout: Duration) -> Option<LiveFrame> {
        let slot = self.slot.lock().unwrap();
        let (mut slot, _) = self
            .ready
            .wait_timeout_while(slot, timeout, |s| s.is_none())
            .unwrap();
        slot.take()
    }

    /// How long ago the newest frame arrived; `None` before the first.
    pub fn age(&self) -> Option<Duration> {
        let at = self.last_at_ms.load(Ordering::SeqCst);
        if at == u64::MAX {
            return None;
        }
        let now = self.epoch.elapsed().as_millis() as u64;
        Some(Duration::from_millis(now.saturating_sub(at)))
    }
}

/// Something producing frames (or samples) until dropped.
pub trait CaptureHandle: Send {}

// ---------------------------------------------------------------------------
// Live audio: a camera's mic into the bus its element declares (PR C).
// ---------------------------------------------------------------------------

/// The most a live input may lag before its oldest samples are dropped
/// (100 ms), and what is kept when that happens (40 ms).
pub const LIVE_AUDIO_MAX_BACKLOG_FRAMES: u64 = 4_800;
pub const LIVE_AUDIO_KEEP_FRAMES: u64 = 1_920;

/// One live audio input: 48 kHz stereo float, pushed by the capture side,
/// read by the audio graph once a block. The graph never waits: a ring the
/// device has not filled reads as silence (counted as underrun frames), and a
/// backlog past `LIVE_AUDIO_MAX_BACKLOG_FRAMES` is trimmed (counted).
pub struct LiveAudio {
    inner: Mutex<(crate::audio_ring::RingConsumer, Vec<f32>)>,
    pub frames_in: AtomicU64,
    pub underrun_frames: AtomicU64,
    pub trimmed_frames: AtomicU64,
}

impl std::fmt::Debug for LiveAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveAudio")
            .field("frames_in", &self.frames_in.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl LiveAudio {
    /// A new input with a one-second ring, and its producer end.
    pub fn new() -> (Arc<Self>, crate::audio_ring::RingProducer) {
        let (producer, consumer) = crate::audio_ring::ring(48_000);
        (
            Arc::new(Self {
                // Scratch for one block, sized once: the audio thread does not allocate.
                inner: Mutex::new((consumer, vec![0.0; 48_000 * crate::audio::CHANNELS])),
                frames_in: AtomicU64::new(0),
                underrun_frames: AtomicU64::new(0),
                trimmed_frames: AtomicU64::new(0),
            }),
            producer,
        )
    }

    /// The audio graph's read: add what arrived, at `gain`, into `out`.
    pub fn mix_into(&self, out: &mut [f32], gain: f32) {
        let Ok(mut inner) = self.inner.try_lock() else {
            return;
        };
        let (consumer, scratch) = &mut *inner;
        let fill = consumer.fill_frames();
        if fill > LIVE_AUDIO_MAX_BACKLOG_FRAMES {
            let dropped = consumer.skip_frames(fill - LIVE_AUDIO_KEEP_FRAMES);
            self.trimmed_frames.fetch_add(dropped, Ordering::Relaxed);
        }
        let want = out.len().min(scratch.len());
        let got = consumer.pop_into(&mut scratch[..want]);
        for (o, v) in out[..got].iter_mut().zip(&scratch[..got]) {
            *o += v * gain;
        }
        if consumer.started() && got < want {
            self.underrun_frames.fetch_add(
                ((want - got) / crate::audio::CHANNELS) as u64,
                Ordering::Relaxed,
            );
        }
    }
}

static LIVE_AUDIO: std::sync::OnceLock<Mutex<HashMap<String, Arc<LiveAudio>>>> =
    std::sync::OnceLock::new();

fn live_audio_registry() -> &'static Mutex<HashMap<String, Arc<LiveAudio>>> {
    LIVE_AUDIO.get_or_init(Default::default)
}

/// The id a camera's mic is resolved by at a take: `live:camera:<cameraId>`.
pub fn live_audio_id(camera_id: &str) -> String {
    format!("live:{}", camera_source_id(camera_id))
}

/// The running live audio input for a `live:` id, if any. Looked up once per
/// take by the audio thread, never per block.
pub fn live_audio(id: &str) -> Option<Arc<LiveAudio>> {
    if !id.starts_with("live:") {
        return None;
    }
    live_audio_registry().lock().unwrap().get(id).cloned()
}

/// `NBE_MICS`' mapping for one camera id: `default`, an input device name
/// (substring), or `synthetic:<hz>`. Unmapped means the camera has no mic.
pub fn mic_device_for(camera_id: &str) -> Option<String> {
    std::env::var("NBE_MICS").ok().and_then(|map| {
        map.split(',').find_map(|pair| {
            let (id, dev) = pair.split_once('=')?;
            (id.trim() == camera_id).then(|| dev.trim().to_string())
        })
    })
}

fn open_mic(
    device: &str,
    producer: crate::audio_ring::RingProducer,
    input: Arc<LiveAudio>,
) -> Result<Box<dyn CaptureHandle>, String> {
    if let Some(hz) = device.strip_prefix("synthetic:") {
        let hz: f32 = hz
            .parse()
            .map_err(|_| format!("bad synthetic mic {device:?}"))?;
        return Ok(Box::new(SyntheticMic::start(hz, producer, input)));
    }
    open_device_mic(device, producer, input)
}

/// A test mic: a sine at -20 dBFS per channel, pushed in 10 ms chunks paced
/// against the wall clock.
pub struct SyntheticMic {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaptureHandle for SyntheticMic {}

impl SyntheticMic {
    fn start(
        hz: f32,
        mut producer: crate::audio_ring::RingProducer,
        input: Arc<LiveAudio>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("nbe-synthetic-mic".into())
            .spawn(move || {
                let start = Instant::now();
                let mut n: u64 = 0;
                let mut chunk = vec![0.0f32; 480 * crate::audio::CHANNELS];
                while !stop2.load(Ordering::SeqCst) {
                    for f in 0..480 {
                        let t = (n + f as u64) as f32 / 48_000.0;
                        let v = 0.1 * (2.0 * std::f32::consts::PI * hz * t).sin();
                        chunk[f * 2] = v;
                        chunk[f * 2 + 1] = v;
                    }
                    let pushed = producer.push(&chunk);
                    input
                        .frames_in
                        .fetch_add((pushed / crate::audio::CHANNELS) as u64, Ordering::Relaxed);
                    n += 480;
                    let due = start + Duration::from_secs_f64(n as f64 / 48_000.0);
                    if let Some(wait) = due.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }
                }
            })
            .expect("spawn the synthetic mic");
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for SyntheticMic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A device mic through cpal, on a thread that owns the stream (a cpal
/// stream is not `Send`). Captured at the device's own rate and channel
/// count, converted to 48 kHz stereo by linear interpolation.
#[cfg(target_os = "macos")]
fn open_device_mic(
    device: &str,
    mut producer: crate::audio_ring::RingProducer,
    input: Arc<LiveAudio>,
) -> Result<Box<dyn CaptureHandle>, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    struct DeviceMic {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl CaptureHandle for DeviceMic {}
    impl Drop for DeviceMic {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<String, String>>();
    let device = device.to_string();
    let thread = std::thread::Builder::new()
        .name("nbe-mic".into())
        .spawn(move || {
            let host = cpal::default_host();
            let dev = if device == "default" {
                host.default_input_device()
            } else {
                host.input_devices().ok().and_then(|mut it| {
                    it.find(|d| {
                        d.description()
                            .map(|x| x.name().contains(device.as_str()))
                            .unwrap_or(false)
                    })
                })
            };
            let Some(dev) = dev else {
                let _ = ready_tx.send(Err(format!("no input device {device:?}")));
                return;
            };
            let name = dev
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_default();
            let config = match dev.default_input_config() {
                Ok(c) if c.sample_format() == cpal::SampleFormat::F32 => c,
                Ok(c) => {
                    let _ = ready_tx.send(Err(format!(
                        "input {name:?} is {:?}, not f32",
                        c.sample_format()
                    )));
                    return;
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("input {name:?}: {e}")));
                    return;
                }
            };
            let channels = config.channels() as usize;
            let rate = config.sample_rate() as f64;
            let step = rate / 48_000.0;
            let mut pos = 0.0f64; // read position in the device's frames, within `carry`
            let mut carry: Vec<f32> = Vec::new(); // device frames not yet consumed, mono
            let mut out: Vec<f32> = Vec::with_capacity(48_000);
            let input2 = input.clone();
            let data = move |buf: &[f32], _: &cpal::InputCallbackInfo| {
                for frame in buf.chunks_exact(channels.max(1)) {
                    carry.push(frame.iter().sum::<f32>() / channels.max(1) as f32);
                }
                out.clear();
                while pos + 1.0 < carry.len() as f64 {
                    let i = pos as usize;
                    let frac = (pos - i as f64) as f32;
                    let v = carry[i] * (1.0 - frac) + carry[i + 1] * frac;
                    out.push(v);
                    out.push(v);
                    pos += step;
                }
                let used = pos as usize;
                carry.drain(..used.min(carry.len()));
                pos -= used as f64;
                let pushed = producer.push(&out);
                input2
                    .frames_in
                    .fetch_add((pushed / crate::audio::CHANNELS) as u64, Ordering::Relaxed);
            };
            let stream = match dev.build_input_stream(
                config.into(),
                data,
                |e| tracing::warn!(error = %e, "mic input error"),
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("input {name:?}: {e}")));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("input {name:?}: {e}")));
                return;
            }
            let _ = ready_tx.send(Ok(format!("{name} at {rate} Hz, {channels} ch")));
            while !stop2.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(50));
            }
            drop(stream);
        })
        .map_err(|e| e.to_string())?;
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(desc)) => {
            tracing::info!(device = %desc, "mic input started");
            Ok(Box::new(DeviceMic {
                stop,
                thread: Some(thread),
            }))
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err("mic input did not start within 5 s".into()),
    }
}

#[cfg(not(target_os = "macos"))]
fn open_device_mic(
    _device: &str,
    _producer: crate::audio_ring::RingProducer,
    _input: Arc<LiveAudio>,
) -> Result<Box<dyn CaptureHandle>, String> {
    Err("mic capture is macOS-only in this build".into())
}

/// A running video source: its mailbox and what keeps it producing.
type VideoSource = (Arc<FrameMailbox>, Box<dyn CaptureHandle>);
/// A running mic: its input and what keeps it producing.
type MicSource = (Arc<LiveAudio>, Box<dyn CaptureHandle>);

/// The running live sources, keyed `camera:<cameraId>`.
#[derive(Default)]
pub struct LiveInputs {
    sources: Mutex<HashMap<String, VideoSource>>,
    /// Running mics, keyed by their `live:` id.
    mics: Mutex<HashMap<String, MicSource>>,
    /// The render loop's frame synchronisers, published once a frame for
    /// telemetry: source id → (sync, frames uploaded).
    pub sync: Mutex<std::collections::BTreeMap<String, (FrameSync, u64)>>,
}

impl std::fmt::Debug for LiveInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let keys: Vec<String> = self.sources.lock().unwrap().keys().cloned().collect();
        f.debug_struct("LiveInputs")
            .field("sources", &keys)
            .finish()
    }
}

/// The layer id a camera element's frames are drawn from.
pub fn camera_source_id(camera_id: &str) -> String {
    format!("camera:{camera_id}")
}

impl LiveInputs {
    /// The mailbox for a source, if it is running.
    pub fn mailbox(&self, source_id: &str) -> Option<Arc<FrameMailbox>> {
        self.sources
            .lock()
            .unwrap()
            .get(source_id)
            .map(|(m, _)| m.clone())
    }

    /// Make exactly these cameras run: start the missing ones, stop the rest.
    /// Called at `show.load` with the package's `cameraId`s. A camera that
    /// fails to open is logged and left absent: its layer draws nothing and
    /// its loss engages the slate when it goes on air, as a lost camera does.
    pub fn ensure_cameras(&self, camera_ids: &[String]) {
        let wanted: Vec<String> = camera_ids.iter().map(|c| camera_source_id(c)).collect();
        let mut sources = self.sources.lock().unwrap();
        sources.retain(|k, _| {
            let keep = wanted.contains(k);
            if !keep {
                tracing::info!(source = %k, "live source stopped (not in the package)");
            }
            keep
        });
        drop(sources);
        self.ensure_mics(camera_ids);
        let mut sources = self.sources.lock().unwrap();
        for camera_id in camera_ids {
            let key = camera_source_id(camera_id);
            if sources.contains_key(&key) {
                continue;
            }
            let device = camera_device_for(camera_id);
            let mailbox = Arc::new(FrameMailbox::default());
            match open_camera(&device, mailbox.clone()) {
                Ok(handle) => {
                    tracing::info!(source = %key, device = %device, "live source started");
                    sources.insert(key, (mailbox, handle));
                }
                Err(e) => {
                    tracing::warn!(source = %key, device = %device, error = %e, "live source failed to open");
                }
            }
        }
    }

    /// Each camera's mic, per `NBE_MICS`; a camera with no mapping has none.
    fn ensure_mics(&self, camera_ids: &[String]) {
        let wanted: Vec<String> = camera_ids
            .iter()
            .filter(|c| mic_device_for(c).is_some())
            .map(|c| live_audio_id(c))
            .collect();
        let mut mics = self.mics.lock().unwrap();
        mics.retain(|k, _| wanted.contains(k));
        live_audio_registry()
            .lock()
            .unwrap()
            .retain(|k, _| wanted.contains(k));
        for camera_id in camera_ids {
            let Some(device) = mic_device_for(camera_id) else {
                continue;
            };
            let key = live_audio_id(camera_id);
            if mics.contains_key(&key) {
                continue;
            }
            let (input, producer) = LiveAudio::new();
            match open_mic(&device, producer, input.clone()) {
                Ok(handle) => {
                    tracing::info!(source = %key, device = %device, "live mic started");
                    live_audio_registry()
                        .lock()
                        .unwrap()
                        .insert(key.clone(), input.clone());
                    mics.insert(key, (input, handle));
                }
                Err(e) => {
                    tracing::warn!(source = %key, device = %device, error = %e, "live mic failed to open")
                }
            }
        }
    }

    /// Stop every source (the package was forgotten).
    pub fn stop_all(&self) {
        self.sources.lock().unwrap().clear();
        self.mics.lock().unwrap().clear();
        live_audio_registry().lock().unwrap().clear();
        self.sync.lock().unwrap().clear();
    }

    /// The telemetry line: one entry per running source.
    pub fn lines(&self) -> Vec<nbe_protocol::LiveInputLine> {
        let sync = self.sync.lock().unwrap().clone();
        let mut out: Vec<nbe_protocol::LiveInputLine> = self
            .sources
            .lock()
            .unwrap()
            .iter()
            .map(|(id, (m, _))| {
                let (fs, uploaded) = sync.get(id).cloned().unwrap_or_default();
                let mic = self
                    .mics
                    .lock()
                    .unwrap()
                    .get(&format!("live:{id}"))
                    .map(|(a, _)| a.clone());
                let read = |f: fn(&LiveAudio) -> &AtomicU64| {
                    mic.as_ref()
                        .map(|a| f(a).load(Ordering::Relaxed))
                        .unwrap_or(0)
                };
                nbe_protocol::LiveInputLine {
                    mic: mic.is_some(),
                    mic_frames_in: read(|a| &a.frames_in),
                    mic_underrun_frames: read(|a| &a.underrun_frames),
                    mic_trimmed_frames: read(|a| &a.trimmed_frames),
                    source: id.clone(),
                    frames_in: m.frames_in.load(Ordering::Relaxed),
                    frames_overwritten: m.frames_overwritten.load(Ordering::Relaxed),
                    frames_uploaded: uploaded,
                    drawn_seq: fs.drawn_seq,
                    repeats: fs.repeats,
                    skips: fs.skips,
                    age_ms: m.age().map(|a| a.as_secs_f64() * 1000.0).unwrap_or(-1.0),
                }
            })
            .collect();
        out.sort_by(|a, b| a.source.cmp(&b.source));
        out
    }
}

/// `NBE_CAMERAS`' mapping for one camera id; `default` when unmapped.
pub fn camera_device_for(camera_id: &str) -> String {
    std::env::var("NBE_CAMERAS")
        .ok()
        .and_then(|map| {
            map.split(',').find_map(|pair| {
                let (id, dev) = pair.split_once('=')?;
                (id.trim() == camera_id).then(|| dev.trim().to_string())
            })
        })
        .unwrap_or_else(|| "default".to_string())
}

fn open_camera(device: &str, mailbox: Arc<FrameMailbox>) -> Result<Box<dyn CaptureHandle>, String> {
    if device == "synthetic" {
        return Ok(Box::new(SyntheticCamera::start(
            mailbox,
            SyntheticConfig::from_env(),
        )));
    }
    open_device_camera(device, mailbox)
}

#[cfg(target_os = "macos")]
fn open_device_camera(
    device: &str,
    mailbox: Arc<FrameMailbox>,
) -> Result<Box<dyn CaptureHandle>, String> {
    struct Device(#[allow(dead_code)] nbe_decode::capture::CameraSession);
    impl CaptureHandle for Device {}
    let session = nbe_decode::capture::open_camera(
        device,
        Box::new(move |width, height, bgra: Vec<u8>, seq| {
            mailbox.publish(LiveFrame {
                width,
                height,
                bgra,
                seq,
            });
        }),
    )?;
    Ok(Box::new(Device(session)))
}

#[cfg(not(target_os = "macos"))]
fn open_device_camera(
    _device: &str,
    _mailbox: Arc<FrameMailbox>,
) -> Result<Box<dyn CaptureHandle>, String> {
    Err("camera capture is macOS-only in this build".into())
}

/// The synthetic camera's shape: what it draws and when it stalls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyntheticConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Stop producing after this many frames, for `stall_ms`, then resume.
    pub stall_after: Option<u64>,
    pub stall_ms: u64,
    /// Each frame is published up to this many milliseconds early or late
    /// (a fixed pseudo-random sequence), so its arrivals straddle the house
    /// clock's ticks as a real camera's do. 0: exactly on its period.
    pub jitter_ms: u64,
}

impl SyntheticConfig {
    /// 1280x720 at 30 fps; `NBE_SYNTHETIC_CAMERA_STALL=<frames>:<ms>` stalls
    /// it, `NBE_SYNTHETIC_CAMERA_JITTER_MS=<ms>` jitters it (capped at 15).
    pub fn from_env() -> Self {
        let jitter_ms = std::env::var("NBE_SYNTHETIC_CAMERA_JITTER_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            .min(15);
        let (stall_after, stall_ms) = std::env::var("NBE_SYNTHETIC_CAMERA_STALL")
            .ok()
            .and_then(|v| {
                let (a, b) = v.split_once(':')?;
                Some((Some(a.parse().ok()?), b.parse().ok()?))
            })
            .unwrap_or((None, 0));
        Self {
            width: 1280,
            height: 720,
            fps: 30,
            stall_after,
            stall_ms,
            jitter_ms,
        }
    }
}

/// A test camera: each frame carries its own sequence number as sixteen
/// black-or-white blocks (most significant bit first) across the middle of
/// the picture, so a recording can be read back frame by frame.
pub struct SyntheticCamera {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaptureHandle for SyntheticCamera {}

/// Where the synthetic camera's code blocks sit, normalized to its frame:
/// sixteen blocks of `CODE_BLOCK` width starting at `CODE_X0`, at `CODE_Y0`.
pub const CODE_X0: f32 = 0.1;
pub const CODE_Y0: f32 = 0.45;
pub const CODE_BLOCK: f32 = 0.05;

impl SyntheticCamera {
    pub fn start(mailbox: Arc<FrameMailbox>, cfg: SyntheticConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("nbe-synthetic-camera".into())
            .spawn(move || {
                let period = Duration::from_secs_f64(1.0 / cfg.fps.max(1) as f64);
                let start = Instant::now();
                let mut seq = 0u64;
                let mut stalled_for = Duration::ZERO;
                let mut lcg: u64 = 0x2545_F491_4F6C_DD1D;
                while !stop2.load(Ordering::SeqCst) {
                    seq += 1;
                    if cfg.stall_after == Some(seq - 1) && cfg.stall_ms > 0 {
                        tracing::info!(
                            after = seq - 1,
                            ms = cfg.stall_ms,
                            "synthetic camera stalls"
                        );
                        let d = Duration::from_millis(cfg.stall_ms);
                        std::thread::sleep(d);
                        stalled_for += d;
                        tracing::info!("synthetic camera resumes");
                    }
                    mailbox.publish(LiveFrame {
                        width: cfg.width,
                        height: cfg.height,
                        bgra: synthetic_frame(cfg.width, cfg.height, seq),
                        seq,
                    });
                    // Paced against the start, so the rate does not drift; the
                    // next frame's own jitter is applied to its due time.
                    let mut due = start + stalled_for + period * seq as u32;
                    if cfg.jitter_ms > 0 {
                        lcg = lcg
                            .wrapping_mul(6_364_136_223_846_793_005)
                            .wrapping_add(1_442_695_040_888_963_407);
                        let span = 2 * cfg.jitter_ms + 1;
                        let off = (lcg >> 33) % span;
                        due += Duration::from_millis(off);
                        due = due
                            .checked_sub(Duration::from_millis(cfg.jitter_ms))
                            .unwrap_or(due);
                    }
                    if let Some(wait) = due.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }
                }
            })
            .expect("spawn the synthetic camera");
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for SyntheticCamera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One synthetic frame: dark grey, the code blocks, and a bar that moves.
pub fn synthetic_frame(width: u32, height: u32, seq: u64) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut px = vec![0u8; w * h * 4];
    for p in px.as_chunks_mut::<4>().0 {
        *p = [64, 64, 64, 255];
    }
    let block = (CODE_BLOCK * w as f32) as usize;
    let x0 = (CODE_X0 * w as f32) as usize;
    let y0 = (CODE_Y0 * h as f32) as usize;
    for bit in 0..16usize {
        let on = (seq >> (15 - bit)) & 1 == 1;
        let v = if on { 255 } else { 0 };
        for y in y0..(y0 + block).min(h) {
            for x in (x0 + bit * block)..(x0 + (bit + 1) * block).min(w) {
                let i = (y * w + x) * 4;
                px[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
    }
    // A moving bar (blue in BGRA) along the bottom, one block a frame.
    let bar_x = (seq as usize * 8) % w.max(1);
    for y in (h * 8 / 10)..(h * 9 / 10) {
        for x in bar_x..(bar_x + w / 20).min(w) {
            let i = (y * w + x) * 4;
            px[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    px
}

/// One source's three textures and the camera frame each holds, written by
/// the upload thread and read by the render loop with atomic loads.
pub struct LiveTextureRing {
    textures: Mutex<Vec<wgpu::Texture>>,
    /// The camera sequence number each slot holds; 0 while being written.
    slot_seq: [AtomicU64; 3],
    /// The newest complete upload's sequence number, 0 before the first.
    newest: AtomicU64,
    stop: AtomicBool,
    /// Frames uploaded.
    pub uploaded: AtomicU64,
}

impl LiveTextureRing {
    /// The newest uploaded camera frame's sequence number (0: none yet).
    pub fn newest_seq(&self) -> u64 {
        self.newest.load(Ordering::SeqCst)
    }

    /// The texture holding camera frame `seq`, if it is still in the ring.
    /// Never waits: a ring being reallocated returns `None` for that frame.
    pub fn frame(&self, seq: u64) -> Option<wgpu::Texture> {
        if seq == 0 {
            return None;
        }
        let slot = self
            .slot_seq
            .iter()
            .position(|s| s.load(Ordering::SeqCst) == seq)?;
        self.textures.try_lock().ok()?.get(slot).cloned()
    }
}

/// Which camera frame the View shows this tick, and the count of frames it
/// repeated or skipped to get there: a frame synchroniser between a camera's
/// clock and the house clock.
///
/// The rule is "exactly one camera frame per house frame when one is there":
/// take the frame after the last one drawn if it has been uploaded; repeat
/// the last one if it has not; jump to the one before the newest, skipping
/// the rest, only when two or more are waiting. A camera running at the house
/// rate with jitter therefore settles one frame behind its newest upload and
/// steps by one every tick — the coin flip between a repeat and a skip, at a
/// phase where uploads land on the tick boundary, does not happen. A camera
/// whose clock is slower or faster than the house clock repeats or skips one
/// frame at a time, and every one is counted.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FrameSync {
    pub drawn_seq: u64,
    pub repeats: u64,
    pub skips: u64,
}

impl FrameSync {
    /// Choose this tick's frame given the newest uploaded one. Returns the
    /// sequence number to draw (0: nothing uploaded yet).
    pub fn advance(&mut self, newest: u64) -> u64 {
        if newest == 0 {
            return 0;
        }
        if self.drawn_seq == 0 || newest < self.drawn_seq {
            // First frame, or the source restarted its count.
            self.drawn_seq = newest;
            return newest;
        }
        let next = self.drawn_seq + 1;
        if next > newest {
            self.repeats += 1;
        } else if newest - next >= 2 {
            self.skips += newest - 1 - next;
            self.drawn_seq = newest - 1;
        } else {
            self.drawn_seq = next;
        }
        self.drawn_seq
    }
}

/// The upload thread for one source.
pub struct LiveUploader {
    pub ring: Arc<LiveTextureRing>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LiveUploader {
    pub fn start(
        source_id: String,
        mailbox: Arc<FrameMailbox>,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Self {
        let ring = Arc::new(LiveTextureRing {
            textures: Mutex::new(Vec::new()),
            slot_seq: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
            newest: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            uploaded: AtomicU64::new(0),
        });
        let r = ring.clone();
        let thread = std::thread::Builder::new()
            .name(format!("nbe-live-upload-{source_id}"))
            .spawn(move || {
                let mut dims = (0u32, 0u32);
                let mut slot = 0usize;
                while !r.stop.load(Ordering::SeqCst) {
                    let Some(frame) = mailbox.take(Duration::from_millis(50)) else {
                        continue;
                    };
                    if frame.bgra.len() != (frame.width * frame.height * 4) as usize {
                        continue;
                    }
                    if dims != (frame.width, frame.height) {
                        let made: Vec<wgpu::Texture> = (0..3)
                            .map(|_| {
                                make_live_texture(&device, frame.width, frame.height, &source_id)
                            })
                            .collect();
                        *r.textures.lock().unwrap() = made;
                        for s in &r.slot_seq {
                            s.store(0, Ordering::SeqCst);
                        }
                        r.newest.store(0, Ordering::SeqCst);
                        dims = (frame.width, frame.height);
                    }
                    slot = (slot + 1) % 3;
                    // The slot is invalid while it is rewritten; the render
                    // loop never chooses a frame older than the newest but
                    // one, which is never in this slot.
                    r.slot_seq[slot].store(0, Ordering::SeqCst);
                    let tex = r.textures.lock().unwrap()[slot].clone();
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &tex,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &frame.bgra,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(frame.width * 4),
                            rows_per_image: Some(frame.height),
                        },
                        wgpu::Extent3d {
                            width: frame.width,
                            height: frame.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    let seq = frame.seq.max(1);
                    r.slot_seq[slot].store(seq, Ordering::SeqCst);
                    r.newest.store(seq, Ordering::SeqCst);
                    r.uploaded.fetch_add(1, Ordering::Relaxed);
                }
            })
            .expect("spawn a live upload thread");
        Self {
            ring,
            thread: Some(thread),
        }
    }
}

impl Drop for LiveUploader {
    fn drop(&mut self) {
        self.ring.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn make_live_texture(device: &wgpu::Device, width: u32, height: u32, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // The camera's own byte order: no swizzle on the CPU, and the sampler
        // reads it as colour like any other texture.
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_carries_its_sequence_number_in_the_code_blocks() {
        let (w, h) = (1280u32, 720u32);
        let px = synthetic_frame(w, h, 0b1010_0000_0000_0011);
        let block = (CODE_BLOCK * w as f32) as usize;
        let x0 = (CODE_X0 * w as f32) as usize;
        let y = (CODE_Y0 * h as f32) as usize + block / 2;
        let mut read = 0u64;
        for bit in 0..16 {
            let x = x0 + bit * block + block / 2;
            let v = px[(y * w as usize + x) * 4];
            read = (read << 1) | u64::from(v > 128);
        }
        assert_eq!(read, 0b1010_0000_0000_0011);
    }

    #[test]
    fn the_mailbox_keeps_only_the_newest_frame_and_counts_the_rest() {
        let m = FrameMailbox::default();
        assert!(m.age().is_none());
        for seq in 1..=3 {
            m.publish(LiveFrame {
                width: 1,
                height: 1,
                bgra: vec![0; 4],
                seq,
            });
        }
        assert_eq!(m.take(Duration::from_millis(1)).map(|f| f.seq), Some(3));
        assert_eq!(m.frames_overwritten.load(Ordering::Relaxed), 2);
        assert!(m.take(Duration::from_millis(1)).is_none());
        assert!(m.age().is_some());
    }

    #[test]
    fn frame_sync_steps_by_one_through_jitter_and_counts_what_it_cannot() {
        let mut fs = FrameSync::default();
        assert_eq!(fs.advance(0), 0);
        assert_eq!(fs.advance(5), 5); // first frame
                                      // Uploads land either side of the tick: newest is 6 or 7.
        let picks: Vec<u64> = [6, 7, 7, 8, 9, 10, 10, 11]
            .into_iter()
            .map(|n| fs.advance(n))
            .collect();
        assert_eq!(picks, vec![6, 7, 7, 8, 9, 10, 10, 11]);
        assert_eq!((fs.repeats, fs.skips), (2, 0));
        // Falling three behind jumps to one before the newest.
        assert_eq!(fs.advance(15), 14);
        assert_eq!(fs.skips, 2);
    }

    #[test]
    fn an_unmapped_camera_opens_the_default_device() {
        // Not set in the test environment for this id.
        assert_eq!(camera_device_for("an-id-nobody-maps"), "default");
    }
}
