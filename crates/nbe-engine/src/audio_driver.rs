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
//! which is what CI runs; the device sink is the recorded deferral in
//! `agents/prompts/06-audio-graph.md`. Everything above the sink — the graph,
//! the drain, the counters, the drift measurement — is real either way.

use crate::audio::{AudioGraph, CHANNELS, SAMPLE_RATE};
use crate::audio_control::{self, AudioCommand};
use crate::state::SharedEngineState;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

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
        }
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

    /// One cycle: drain intents, render a block, publish measurements.
    ///
    /// Returns the number of frames rendered.
    pub fn cycle(&mut self, master_frame: u64) -> usize {
        self.sync_library();

        // Drain a bounded batch of intents. The lock is held only long enough
        // to move them out — the directive thread is never blocked behind a
        // render.
        let batch: Vec<AudioCommand> = {
            let mut pending = self.state.audio_commands.lock().unwrap();
            let take = pending.len().min(MAX_COMMANDS_PER_CYCLE);
            pending.drain(..take).collect()
        };
        if !batch.is_empty() {
            audio_control::apply(&mut self.graph, batch, &self.library);
        }

        self.graph.render(&mut self.block, master_frame);
        if !self.sink.write(&self.block) {
            // SPEC §8.10: a block the sink could not take is an underrun. It
            // is counted and never touches the video watchdog.
            self.graph.note_underrun();
        }

        self.publish(master_frame);
        self.block.len() / CHANNELS
    }

    /// Publish the v0.3.3 telemetry fields (SPEC §10.1).
    ///
    /// Peaks are **windowed**: published then reset, so each telemetry tick
    /// reports the peak of the interval it covers rather than the loudest
    /// moment since the show began. That is the broadcast norm — a meter that
    /// never falls tells an operator nothing after the first transient.
    fn publish(&mut self, master_frame: u64) {
        self.state
            .audio_underruns_total
            .store(self.graph.underruns(), Ordering::SeqCst);
        self.state.audio_drift_ms_bits.store(
            self.graph.drift_ms(master_frame).to_bits(),
            Ordering::SeqCst,
        );
        *self.state.bus_peaks.lock().unwrap() = self.graph.bus_peaks();
        self.graph.reset_meters();
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
pub fn spawn(state: SharedEngineState, house_rate: u32) -> tokio::task::JoinHandle<()> {
    let block_frames = SAMPLE_RATE as usize / house_rate.max(1) as usize;
    let driver = AudioDriver::new(
        state.clone(),
        Box::new(NullSink::new(block_frames)),
        house_rate,
    );
    tokio::spawn(run(driver, state))
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
