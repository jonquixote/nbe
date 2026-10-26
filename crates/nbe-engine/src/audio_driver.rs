//! The audio driver (Prompt 06b Step 2).
//!
//! A graph nothing drives is a mechanism test. This is what makes the engine
//! audible: it owns the `AudioGraph`, drains the intents the directive path
//! publishes, renders blocks on the master-clock cadence, and publishes the
//! SPEC v0.3.3 telemetry fields into `EngineState` — so
//! `audioUnderrunsTotal`, `audioDriftMs`, and `busPeakDbfs` are measurements
//! written by production code rather than stubs a test filled in.
//!
//! The sink is deliberately abstract. A `NullSink` consumes blocks on cadence,
//! which is what CI runs. The device sink (`audio_device`, macOS, the
//! device-audio design record D1–D2) pushes into a lock-free ring the device's
//! callback copies out of: the driver renders from its own cursor whenever the
//! ring's fill falls below target, aligned to the master clock through the
//! callback's presentation marks (`audio_clock`). Everything above the sink —
//! the graph, the drain, the counters — is the same either way; the drift a
//! device sink publishes is the device's (SPEC §8.9, R27).

use crate::audio::{AudioGraph, CHANNELS, SAMPLE_RATE};
use crate::audio_clock::{DeviceMarks, Feeder};
use crate::audio_control::{self, AudioCommand};
use crate::audio_ring::RingProducer;
use crate::state::{SharedEngineState, SharedOutgoing};
use nbe_core::automation::{AudioLevelWatch, CrossingDirection, LEVEL_FLOOR_DBFS};
use nbe_protocol::EngineFrame;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The meter window, in milliseconds (SPEC §10.1's telemetry cadence).
///
/// Peaks are held across this span and published on its boundary, so a tick
/// reports the loudest moment of the interval it covers.
const DEFAULT_METER_WINDOW_MS: u64 = 1000;

/// Intents applied per cycle.
///
/// Bounded because the drain runs on the audio cadence: an unbounded drain
/// turns a flood of commands into a missed block, which is an underrun the
/// operator hears.
pub const MAX_COMMANDS_PER_CYCLE: usize = 64;

/// Where rendered audio goes.
pub trait AudioSink: Send {
    /// Consume one block. Returning `false` means the sink could not take it,
    /// which the driver counts as an underrun (SPEC §8.10).
    fn write(&mut self, block: &[f32]) -> bool;
    /// Block size in sample frames.
    fn block_frames(&self) -> usize;
    /// The device side, when this sink is a device (D2): the ring the driver
    /// pushes into and the marks the callback publishes. `None` for a sink the
    /// driver feeds on its own cadence.
    fn device(&mut self) -> Option<&mut DeviceIo> {
        None
    }
}

/// What a device sink hands the driver.
pub struct DeviceIo {
    pub producer: RingProducer,
    pub marks: Arc<DeviceMarks>,
    pub info: DeviceInfo,
}

/// What was opened.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
    pub rate_hz: u32,
    /// The device's rate before the sink switched it to 48 kHz, restored at
    /// close; `None` when it already ran at 48 kHz (D1 a).
    pub prior_rate_hz: Option<f64>,
    /// The HAL's full output latency in frames: the device's own, its safety
    /// offset, and its stream's (D1 b).
    pub latency_frames: u32,
}

/// The ring's fill the driver holds, in frames: two house-rate blocks. Below
/// it the driver renders another block. The fill is latency the cursor
/// compensates, and margin the audio thread spends when it is late.
pub const TARGET_FILL_BLOCKS: u64 = 2;

/// The ring's capacity, in blocks.
pub const RING_BLOCKS: usize = 8;

/// How often a device-mode driver checks the ring.
pub const DEVICE_POLL: Duration = Duration::from_millis(4);

/// A device that has stopped calling back for this long is lost (SPEC
/// §8.10's device loss), even if it never said so.
pub const DEVICE_SILENT_LIMIT: Duration = Duration::from_millis(1000);

/// Underruns closer together than this are one episode; only an episode's
/// first is logged (SPEC §8.10).
pub const EPISODE_GAP: Duration = Duration::from_millis(1000);

/// The rate guard's decision (D1 a): the rate is the engine's explicit act,
/// never cpal's side effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RatePlan {
    /// Already 48 kHz: open, change nothing.
    Keep,
    /// Switch to 48 kHz now, restore `prior_hz` at close.
    Switch { prior_hz: f64 },
    /// The device cannot run at 48 kHz: refuse it (`E_AUDIO`), and the
    /// engine falls back to silence.
    Refuse,
}

/// Decide what the rate guard does with a device at `nominal_hz`.
pub fn rate_plan(nominal_hz: f64, supports_48k: bool) -> RatePlan {
    if nominal_hz == SAMPLE_RATE as f64 {
        RatePlan::Keep
    } else if supports_48k {
        RatePlan::Switch {
            prior_hz: nominal_hz,
        }
    } else {
        RatePlan::Refuse
    }
}

/// Which sink the engine runs (D7): `NBE_AUDIO_SINK=device|null`, `device`
/// by default. CI and every test harness set `null`, so their audio stays
/// deterministic whether or not a runner has a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkChoice {
    Device,
    Null,
    /// A value neither word: the engine runs silent and says why.
    Unknown(String),
}

pub fn sink_choice(var: Option<&str>) -> SinkChoice {
    match var.map(str::trim) {
        None | Some("") | Some("device") => SinkChoice::Device,
        Some("null") => SinkChoice::Null,
        Some(other) => SinkChoice::Unknown(other.to_string()),
    }
}

/// A sink that consumes blocks and keeps the count. What CI runs, and what a
/// soak test measures drift against.
#[derive(Debug, Default)]
pub struct NullSink {
    pub blocks: u64,
    pub frames: u64,
    block_frames: usize,
    /// Set to fail the next write, so the underrun path is reachable in tests.
    pub fail_next: bool,
}

impl NullSink {
    pub fn new(block_frames: usize) -> Self {
        Self {
            block_frames,
            ..Default::default()
        }
    }
}

impl AudioSink for NullSink {
    fn write(&mut self, block: &[f32]) -> bool {
        if self.fail_next {
            self.fail_next = false;
            return false;
        }
        self.blocks += 1;
        self.frames += (block.len() / CHANNELS) as u64;
        true
    }

    fn block_frames(&self) -> usize {
        self.block_frames
    }
}

/// The driver: graph + sink + the state it publishes into.
pub struct AudioDriver {
    pub graph: AudioGraph,
    sink: Box<dyn AudioSink>,
    state: SharedEngineState,
    block: Vec<f32>,
    /// Soundboard samples, resident from `show.load` (SPEC §8.4).
    library: BTreeMap<String, Arc<Vec<f32>>>,
    library_generation: u64,
    /// The record tap currently attached to the graph (WU-pipe). The
    /// directive path publishes the take's shared tap into engine state;
    /// this driver owns the graph, so it attaches/detaches here, once per
    /// cycle, by pointer — no audio-thread cost beyond one `Arc` clone and
    /// comparison.
    attached_tap: Option<Arc<crate::record::AudioTap>>,
    /// The stream tap currently attached to the graph.
    attached_stream_tap: Option<Arc<crate::record::AudioTap>>,
    /// Peaks accumulating inside the current meter window.
    window_peaks: BTreeMap<String, f64>,
    /// Blocks elapsed in the current window, and how many make one.
    window_blocks: u32,
    blocks_per_window: u32,
    /// Where crossings go: the render channel's outbound queue (B1). `None`
    /// for a driver nobody listens to — crossings are still detected and
    /// counted, just not sent.
    events: Option<SharedOutgoing>,
    /// The `audioLevel` watches in force, swapped in by pointer from
    /// `EngineState::audio_level_watches` (set at `show.load`).
    watches: Arc<Vec<AudioLevelWatch>>,
    /// Per watch: was the bus at-or-above the threshold on the last block.
    /// Starts below (a fresh show is silent), so a rule never fires on a
    /// level that was already present when it was installed.
    above: Vec<bool>,
    /// Crossings detected since this driver started — observable to tests
    /// whether or not anyone is listening.
    crossings: u64,
    /// Device mode: the cursor, the slip, the push log (D2).
    pub feeder: Feeder,
    /// Device events already counted: short callbacks and overloads.
    seen_short: u64,
    seen_xruns: u64,
    /// The last underrun, for §8.10's one log line per episode.
    last_underrun: Option<Instant>,
    /// When the device was last seen calling back, and its count then.
    seen_callbacks: u64,
    last_callback_seen: Instant,
    /// The skew the device last presented, ms (§8.9's drift, device-referenced).
    device_skew_ms: Option<f64>,
    /// Blocks the ring could not take whole (the device stopped consuming).
    pub overruns: u64,
}

/// Whether the wanted tap differs from the attached one (by pointer).
fn tap_changed(
    attached: &Option<Arc<crate::record::AudioTap>>,
    wanted: &Option<Arc<crate::record::AudioTap>>,
) -> bool {
    match (attached, wanted) {
        (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
        (None, None) => false,
        _ => true,
    }
}

impl AudioDriver {
    pub fn new(state: SharedEngineState, sink: Box<dyn AudioSink>, house_rate: u32) -> Self {
        let block_frames = sink.block_frames();
        let mut graph = AudioGraph::new(house_rate);
        // Size the scratch buffers once, here, so `render` never grows them on
        // the cadence path (SPEC §8.9).
        graph.reserve(block_frames * CHANNELS);
        Self {
            graph,
            sink,
            state,
            block: vec![0.0; block_frames * CHANNELS],
            library: BTreeMap::new(),
            library_generation: u64::MAX,
            attached_tap: None,
            attached_stream_tap: None,
            window_peaks: BTreeMap::new(),
            window_blocks: 0,
            // One telemetry interval's worth of blocks, so a published meter
            // covers exactly the interval a tick reports.
            blocks_per_window: Self::blocks_per(
                block_frames,
                Duration::from_millis(DEFAULT_METER_WINDOW_MS),
            ),
            events: None,
            watches: Arc::new(Vec::new()),
            above: Vec::new(),
            crossings: 0,
            feeder: Feeder::new(block_frames),
            seen_short: 0,
            seen_xruns: 0,
            last_underrun: None,
            seen_callbacks: 0,
            last_callback_seen: Instant::now(),
            device_skew_ms: None,
            overruns: 0,
        }
    }

    /// Send `audioLevelCrossing` frames to `outgoing` (B1). The production
    /// driver always has this ([`spawn_with_events`]); a test that only
    /// meters can go without.
    pub fn with_events(mut self, outgoing: SharedOutgoing) -> Self {
        self.events = Some(outgoing);
        self
    }

    /// Crossings detected so far.
    pub fn crossings(&self) -> u64 {
        self.crossings
    }

    /// Pick up soundboard assets when a new package loads (SPEC §8.4).
    fn sync_library(&mut self) {
        let generation = self.state.package_generation.load(Ordering::SeqCst);
        if generation == self.library_generation {
            return;
        }
        self.library_generation = generation;
        self.library.clear();
        let audio = self.state.audio_assets.lock().unwrap();
        for (id, samples) in audio.iter() {
            self.library.insert(id.clone(), samples.clone());
        }
        tracing::info!(assets = self.library.len(), "soundboard library resident");
    }

    /// Attach/detach the take's shared record tap (WU-pipe).
    ///
    /// The directive path (`record.start` / stops) publishes into
    /// `state.record_tap`; the graph lives here, so the attach happens here.
    /// Pointer-compared once per cycle: steady state is one `Arc` clone, no
    /// lock held across the render, no deadline change (the tap's `push` is
    /// lock-free by contract — atomic publish, no contention shed).
    fn sync_record_tap(&mut self) {
        let wanted = self.state.record_tap.lock().unwrap().clone();
        if tap_changed(&self.attached_tap, &wanted) {
            match &wanted {
                Some(tap) => self.graph.set_record_tap(tap.clone()),
                None => self.graph.clear_record_tap(),
            }
            self.attached_tap = wanted;
        }
    }

    /// Attach/detach the live stream's tap (`state.stream_tap`), the record
    /// tap's discipline: pointer-compared once per cycle.
    fn sync_stream_tap(&mut self) {
        let wanted = self.state.stream_tap.lock().unwrap().clone();
        if tap_changed(&self.attached_stream_tap, &wanted) {
            match &wanted {
                Some(tap) => self.graph.set_stream_tap(tap.clone()),
                None => self.graph.clear_stream_tap(),
            }
            self.attached_stream_tap = wanted;
        }
    }

    /// One cycle: drain intents, render a block, publish measurements.
    ///
    /// Returns the number of frames rendered.
    pub fn cycle(&mut self, master_frame: u64) -> usize {
        self.sync_library();
        self.sync_record_tap();
        self.sync_stream_tap();
        self.drain_commands();

        self.graph.render(&mut self.block, master_frame);
        if !self.sink.write(&self.block) {
            // SPEC §8.10: a block the sink could not take is an underrun. It
            // is counted and never touches the video watchdog.
            self.graph.note_underrun();
        }

        self.publish(master_frame);
        self.block.len() / CHANNELS
    }

    /// Drain a bounded batch of intents. The lock is held only long enough to
    /// move them out — the directive thread is never blocked behind a render.
    fn drain_commands(&mut self) {
        let batch: Vec<AudioCommand> = {
            let mut pending = self.state.audio_commands.lock().unwrap();
            let take = pending.len().min(MAX_COMMANDS_PER_CYCLE);
            pending.drain(..take).collect()
        };
        if batch.is_empty() {
            return;
        }
        // The ceiling in force reaches the tick (D5's amendment of
        // 2026-10-23): its value and source, read-only, beside the log line
        // `apply` writes.
        for cmd in &batch {
            if let AudioCommand::SetLimiterCeiling { dbtp, source } = cmd {
                *self.state.audio_limiter_ceiling.lock().unwrap() =
                    nbe_protocol::LimiterCeilingLine {
                        dbtp: *dbtp,
                        source: (*source).to_string(),
                    };
            }
        }
        audio_control::apply(&mut self.graph, batch, &self.library);
    }

    /// One device-mode cycle (D2): count what the device reported, then render
    /// while the ring's fill is under target, each block planned against the
    /// device's latest presentation mark. Returns blocks rendered.
    pub fn cycle_device(&mut self) -> usize {
        self.sync_library();
        self.sync_record_tap();
        self.sync_stream_tap();
        self.drain_commands();
        self.collect_device_events();

        let epoch = self.state.clock.lock().unwrap().epoch();
        let block = self.block_frames() as u64;
        let mut rendered = 0;
        while let Some(dev) = self.sink.device() {
            let fill = dev.producer.fill_frames();
            if fill >= TARGET_FILL_BLOCKS * block {
                break;
            }
            let marks = dev.marks.clone();
            let next = dev.producer.written_frames();
            let lead = fill + dev.info.latency_frames as u64;
            let plan = self.feeder.plan(
                epoch.map(|e| marks.signed_ns_of(e)),
                next,
                marks.latest(),
                marks.ns_of(Instant::now()),
                lead,
            );
            self.graph.render_at(&mut self.block, plan.base_sample);
            let out = self.feeder.emit(&self.block, &plan, next);
            let dev = self.sink.device().expect("checked above");
            if dev.producer.push(out) < out.len() {
                // Cannot happen while the device consumes: the fill was under
                // target. Counted, never blocked on.
                self.overruns += 1;
                break;
            }
            rendered += 1;
            self.publish(plan.base_sample.max(0) as u64 / block);
        }
        rendered
    }

    /// Count what the device reported since the last cycle: underruns (short
    /// callbacks and overloads, §8.10), the presented skew (§8.9), and loss.
    fn collect_device_events(&mut self) {
        let Some(dev) = self.sink.device() else {
            return;
        };
        let marks = dev.marks.clone();
        let name = dev.info.name.clone();
        let short = marks.short_callbacks.load(Ordering::Relaxed);
        let xruns = marks.xruns.load(Ordering::Relaxed);
        let new_short = short - self.seen_short;
        let new_xruns = xruns - self.seen_xruns;
        self.seen_short = short;
        self.seen_xruns = xruns;
        let now = Instant::now();
        if new_short + new_xruns > 0 {
            for _ in 0..new_short + new_xruns {
                self.graph.note_underrun();
            }
            let episode = self
                .last_underrun
                .is_none_or(|t| now.duration_since(t) > EPISODE_GAP);
            if episode {
                tracing::warn!(
                    device = %name,
                    short_callbacks = new_short,
                    overloads = new_xruns,
                    total = self.graph.underruns(),
                    "audio underrun episode begins (SPEC §8.10)"
                );
            }
            self.last_underrun = Some(now);
        }
        self.device_skew_ms = marks
            .latest()
            .and_then(|m| self.feeder.presented_skew_ms(m));
        // Straight to the tick: a cycle that renders nothing (the ring at
        // target) still reports what the device did.
        self.state
            .audio_underruns_total
            .store(self.graph.underruns(), Ordering::SeqCst);
        if let Some(ms) = self.device_skew_ms {
            self.state
                .audio_drift_ms_bits
                .store(ms.to_bits(), Ordering::SeqCst);
        }
        let callbacks = marks.callbacks.load(Ordering::Relaxed);
        if callbacks != self.seen_callbacks {
            self.seen_callbacks = callbacks;
            self.last_callback_seen = now;
        }
        let silent = now.duration_since(self.last_callback_seen) > DEVICE_SILENT_LIMIT;
        if marks.lost.load(Ordering::Relaxed) || silent {
            let reason = if silent {
                format!("the device stopped calling back for over {DEVICE_SILENT_LIMIT:?}")
            } else {
                "the device reported itself lost".to_string()
            };
            self.fall_back(nbe_protocol::AudioDeviceLine::lost(&name, &reason));
        }
    }

    /// Lose the device: silence from here on, on the null sink's cadence, so
    /// the graph, the taps and the meters keep running (SPEC §8.10: never
    /// block the render loop; the reopen is U5's, unbuilt).
    fn fall_back(&mut self, line: nbe_protocol::AudioDeviceLine) {
        tracing::error!(
            reason = line.reason.as_deref().unwrap_or(""),
            "audio device lost (E_AUDIO): program audio continues silent"
        );
        *self.state.audio_device.lock().unwrap() = line;
        let block = self.block_frames();
        self.sink = Box::new(NullSink::new(block));
        self.device_skew_ms = None;
    }

    /// Whether this driver is feeding a device.
    pub fn is_device(&mut self) -> bool {
        self.sink.device().is_some()
    }

    /// Publish the v0.3.3 telemetry fields (SPEC §10.1).
    ///
    /// Peaks are **windowed to the telemetry interval, not to the audio
    /// block**. This wrote the block's peaks and reset, every ~33 ms, while
    /// telemetry sampled once per second: each tick therefore reported one
    /// block — about 3% of the interval it claimed to cover — and a 0.4 s
    /// soundboard stab was a coin toss (rehearsal step 6, red in 4 of 6 runs).
    /// An operator saw a strobe, not a level.
    ///
    /// The decision, written down: **peak-hold across the telemetry interval.**
    /// Each block max-merges into the published map; the tick builder takes the
    /// accumulated peak and clears it, so every tick reports the loudest moment
    /// of the interval it covers. Not a decay envelope — a decay needs a time
    /// constant nobody has specified, and hold-and-clear needs none while
    /// answering the question an operator actually asks ("did it peak?").
    fn publish(&mut self, master_frame: u64) {
        self.state
            .audio_underruns_total
            .store(self.graph.underruns(), Ordering::SeqCst);
        // §8.9's drift: the device's presented skew when a device is open
        // (R27); with the null sink, the render cadence's slip, as before.
        let drift = match self.device_skew_ms {
            Some(ms) => ms,
            None if self.sink.device().is_some() => 0.0,
            None => self.graph.drift_ms(master_frame),
        };
        self.state
            .audio_drift_ms_bits
            .store(drift.to_bits(), Ordering::SeqCst);
        // The block's own peaks, read once: the crossing check wants THIS
        // block (render cadence, B1), the telemetry window wants them merged.
        let block_peaks = self.graph.bus_peaks();
        self.detect_crossings(&block_peaks, master_frame);
        for (bus, peak) in block_peaks {
            let slot = self.window_peaks.entry(bus).or_insert(f64::NEG_INFINITY);
            if peak > *slot {
                *slot = peak;
            }
        }
        self.graph.reset_meters();

        // Roll the window over on its own boundary. The published map stays a
        // plain snapshot — no reader has to clear it to make the next one
        // correct, so `bus_peaks` can have more than one consumer.
        self.window_blocks += 1;
        if self.window_blocks >= self.blocks_per_window {
            *self.state.bus_peaks.lock().unwrap() = std::mem::take(&mut self.window_peaks);
            self.window_blocks = 0;
        }
    }

    /// B1 (SPEC v0.4.7 candidate): compare this block's peak on each watched
    /// bus with its threshold, and report a crossing the moment it happens —
    /// on the block, which is one house frame, so the control plane hears of
    /// it within the frame AC-25 #1 allows rather than on the next 1 Hz tick.
    ///
    /// `rising` fires on below → at-or-above; `falling` on at-or-above →
    /// below. No hysteresis: a level hovering on a threshold crosses on every
    /// block it changes side, and the evaluator's once-per-frame limiter
    /// (§13.3 #3) is what bounds the firing, not a band invented here.
    fn detect_crossings(&mut self, block_peaks: &BTreeMap<String, f64>, master_frame: u64) {
        let current = self.state.audio_level_watches.lock().unwrap().clone();
        if !Arc::ptr_eq(&current, &self.watches) {
            self.above = vec![false; current.len()];
            self.watches = current;
        }
        // One stamp per block: every crossing on it was computed now.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        for (i, w) in self.watches.iter().enumerate() {
            let level = block_peaks
                .get(&w.bus)
                .copied()
                .unwrap_or(LEVEL_FLOOR_DBFS)
                .max(LEVEL_FLOOR_DBFS);
            let now_above = level >= w.threshold_dbfs;
            let was_above = std::mem::replace(&mut self.above[i], now_above);
            let crossed = match w.direction {
                CrossingDirection::Rising => !was_above && now_above,
                CrossingDirection::Falling => was_above && !now_above,
            };
            if !crossed {
                continue;
            }
            self.crossings += 1;
            if let Some(q) = &self.events {
                q.push(EngineFrame::AudioLevelCrossing {
                    v: nbe_protocol::PROTOCOL_VERSION.to_string(),
                    ts,
                    bus: w.bus.clone(),
                    threshold_dbfs: w.threshold_dbfs,
                    direction: match w.direction {
                        CrossingDirection::Rising => nbe_protocol::CrossingDirection::Rising,
                        CrossingDirection::Falling => nbe_protocol::CrossingDirection::Falling,
                    },
                    level_dbfs: level,
                    master_frame,
                });
            }
        }
    }

    /// How many blocks of `block_frames` samples span `window`.
    fn blocks_per(block_frames: usize, window: Duration) -> u32 {
        let block = block_frames as f64 / SAMPLE_RATE as f64;
        ((window.as_secs_f64() / block).round() as u32).max(1)
    }

    /// Test seam: shorten the meter window so a boundary is reachable in a
    /// handful of blocks instead of a second of them.
    pub fn set_meter_window(&mut self, window: Duration) {
        self.blocks_per_window = Self::blocks_per(self.block.len() / CHANNELS, window);
        self.window_blocks = 0;
        self.window_peaks.clear();
    }

    /// The wall-clock duration one block represents.
    pub fn block_duration(&self) -> Duration {
        Duration::from_secs_f64((self.block.len() / CHANNELS) as f64 / SAMPLE_RATE as f64)
    }

    pub fn block_frames(&self) -> usize {
        self.block.len() / CHANNELS
    }
}

/// Build the driver the engine runs and spawn it on its own task.
///
/// This exists so the production wiring is one named call rather than eight
/// lines inside `main`: deleting the spawn from `main` used to leave the whole
/// suite green, because nothing can observe `main()`. `spawn` is covered by a
/// test that watches it drain and publish, and `main`'s call to it by a source
/// assertion in `tests/prompt06.rs`.
///
/// The sink is the null sink: device glue is the recorded deferral in
/// `agents/prompts/06-audio-graph.md`. Everything above the sink is real.
/// A running audio thread, and the means to stop it.
///
/// A handle that could not stop the thread would leak one per caller — fine for
/// the engine binary, which runs until the process ends, and not fine for
/// anything else.
pub struct AudioThread {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

impl AudioThread {
    /// Signal the loop to finish its block and return, then join it.
    pub fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.handle.join();
    }
}

pub fn spawn(state: SharedEngineState, house_rate: u32) -> AudioThread {
    spawn_driver(state, house_rate, None, SinkChoice::Null)
}

/// [`spawn`], with `audioLevelCrossing` frames going to the render channel's
/// outbound queue (B1), on the sink `choice` names. What the engine binary
/// runs, with `NBE_AUDIO_SINK`'s choice (D7).
pub fn spawn_with_events(
    state: SharedEngineState,
    house_rate: u32,
    outgoing: SharedOutgoing,
    choice: SinkChoice,
) -> AudioThread {
    spawn_driver(state, house_rate, Some(outgoing), choice)
}

/// Open the sink `choice` names and publish the tick's device line. A device
/// that does not open falls back to silence with its reason on the line: the
/// control plane alerts on it, and nothing refuses (D6).
pub fn open_sink(
    choice: &SinkChoice,
    block_frames: usize,
    state: &crate::state::EngineState,
) -> Box<dyn AudioSink> {
    let null = || -> Box<dyn AudioSink> { Box::new(NullSink::new(block_frames)) };
    let (sink, line) = match choice {
        SinkChoice::Null => (
            null(),
            nbe_protocol::AudioDeviceLine::null("NBE_AUDIO_SINK=null"),
        ),
        SinkChoice::Unknown(v) => (
            null(),
            nbe_protocol::AudioDeviceLine::absent(&format!(
                "NBE_AUDIO_SINK={v:?} is neither \"device\" nor \"null\""
            )),
        ),
        SinkChoice::Device => open_device(block_frames),
    };
    if line.state != "open" {
        tracing::warn!(
            state = %line.state,
            reason = line.reason.as_deref().unwrap_or(""),
            "audio runs silent: no output device carries program audio"
        );
    }
    *state.audio_device.lock().unwrap() = line;
    sink
}

#[cfg(target_os = "macos")]
fn open_device(block_frames: usize) -> (Box<dyn AudioSink>, nbe_protocol::AudioDeviceLine) {
    use crate::audio_device::{DeviceChoice, DeviceSink};
    match DeviceSink::open(&DeviceChoice::Default, block_frames) {
        Ok(sink) => {
            let line = nbe_protocol::AudioDeviceLine::open(&sink.info().name, sink.info().rate_hz);
            (Box::new(sink), line)
        }
        Err(e) => {
            tracing::error!(error = %e, "audio device did not open (E_AUDIO)");
            (
                Box::new(NullSink::new(block_frames)),
                nbe_protocol::AudioDeviceLine::absent(&e.to_string()),
            )
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn open_device(block_frames: usize) -> (Box<dyn AudioSink>, nbe_protocol::AudioDeviceLine) {
    (
        Box::new(NullSink::new(block_frames)),
        nbe_protocol::AudioDeviceLine::absent("no device sink on this platform (D7)"),
    )
}

fn spawn_driver(
    state: SharedEngineState,
    house_rate: u32,
    outgoing: Option<SharedOutgoing>,
    choice: SinkChoice,
) -> AudioThread {
    let block_frames = SAMPLE_RATE as usize / house_rate.max(1) as usize;
    // A DEDICATED OS THREAD, not a task on the shared runtime.
    //
    // This was `tokio::spawn`, which put the audio cadence on the same worker
    // pool as the wgpu render loop and the directive handler. A block is 33 ms;
    // when the render loop spun up at `show.start` the audio task's wakeup
    // slipped past its deadline and §8.10 counted it — finding R4, an underrun
    // on the happy path, reproduced at exactly the moment the clock started and
    // never before it. Measured on the reference machine: underruns 0 through
    // the entire load, then 2 in the tick the first video frames appeared.
    //
    // §8.10's table is explicit that a missed callback deadline IS an underrun,
    // so the counter was right and the scheduling was wrong. Audio cadence is
    // not cooperative work: it cannot yield to a frame and must not wait behind
    // one. `cycle` is synchronous, so this loop needs no executor at all.
    //
    // The sink opens ON this thread: a device's stream is created where it is
    // driven, and closed where it is dropped, so the rate restore (D1 a) runs
    // on this thread when the loop returns.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    let handle = std::thread::Builder::new()
        .name("nbe-audio".into())
        .spawn(move || {
            let sink = open_sink(&choice, block_frames, &state);
            let mut driver = AudioDriver::new(state.clone(), sink, house_rate);
            if let Some(q) = outgoing {
                driver = driver.with_events(q);
            }
            run_blocking(driver, state, &flag)
        })
        .expect("the audio thread must start");
    AudioThread { stop, handle }
}

/// The cadence loop, on its own thread.
///
/// Identical timing to [`run`], which the tests drive; the only difference is
/// which scheduler decides when it wakes.
pub fn run_blocking(
    mut driver: AudioDriver,
    state: SharedEngineState,
    stop: &std::sync::atomic::AtomicBool,
) {
    let block = driver.block_duration();
    let mut next = std::time::Instant::now();
    let mut logged_first = false;
    while !stop.load(Ordering::SeqCst) {
        if driver.is_device() {
            // Device mode (D2): the ring sets the pace, so production follows
            // the device's clock and the slip holds it to the master's. A late
            // wake-up here is absorbed by the ring's fill; only the callback
            // finding the ring short is an underrun.
            driver.cycle_device();
            if !logged_first {
                logged_first = true;
                tracing::info!(
                    rendered_samples = driver.graph.rendered_samples(),
                    "audio driver cycling (device)"
                );
            }
            std::thread::sleep(DEVICE_POLL);
            // If the device is lost, the null cadence resumes from now.
            next = std::time::Instant::now();
            continue;
        }
        let now = std::time::Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        let master_frame = state.master_frame().unwrap_or(0);
        driver.cycle(master_frame);
        if !logged_first {
            logged_first = true;
            tracing::info!(
                rendered_samples = driver.graph.rendered_samples(),
                "audio driver cycling"
            );
        }
        next += block;
        let now = std::time::Instant::now();
        if next < now {
            driver.graph.note_underrun();
            next = now + block;
        }
    }
}

/// Run the driver until the process ends.
///
/// Cadence comes from the master clock: each cycle covers one block, and the
/// loop sleeps until that block's worth of wall time has passed. While the
/// clock is STOPPED the graph still renders — silence has to be produced too,
/// and a sink that stops being fed is a sink that underruns.
pub async fn run(mut driver: AudioDriver, state: SharedEngineState) {
    let block = driver.block_duration();
    let mut next = std::time::Instant::now();
    let mut logged_first = false;
    loop {
        let now = std::time::Instant::now();
        if next > now {
            tokio::time::sleep(next - now).await;
        }
        let master_frame = state.master_frame().unwrap_or(0);
        driver.cycle(master_frame);

        // An operational breadcrumb, not a gate. Three test gates were built
        // on log lines like this one and all three were defeated by writing
        // the line by hand — see the note in tests/prompt06.rs. The wiring is
        // gated by [RI-1]'s dress rehearsal, which observes telemetry.
        if !logged_first {
            logged_first = true;
            tracing::info!(
                rendered_samples = driver.graph.rendered_samples(),
                "audio driver cycling"
            );
        }
        next += block;
        // Fell behind: resynchronize rather than spiral, and count the gap.
        let now = std::time::Instant::now();
        if next < now {
            driver.graph.note_underrun();
            next = now + block;
        }
    }
}
