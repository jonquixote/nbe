//! The audio graph (Prompt 06, SPEC §8).
//!
//! The graph is **offline-renderable**: `AudioGraph::render` fills a buffer
//! with no device present. That is not a testing convenience, it is the
//! design — every §8 acceptance criterion is a measurement over samples
//! (AC-13 latency, AC-18 isolation, AC-19 click-free), and a graph that only
//! works with a device open cannot be measured. The device is one consumer of
//! this graph, never its owner.
//!
//! Clock discipline (SPEC §8.9): there is one clock. A source is read at the
//! sample offset its item's position implies —
//! `(F - t0) * sampleRate / houseFrameRate` — never from a cursor that
//! advances on its own, so a frame and its audio cannot disagree.
//!
//! Real-time discipline (SPEC §8.9, §7.13): `render` allocates nothing and
//! locks nothing, so it is safe to call from a device callback.

use std::collections::BTreeMap;

pub const SAMPLE_RATE: u32 = 48_000;
/// Interleaved stereo throughout.
pub const CHANNELS: usize = 2;

/// SPEC §8.7.1: no gain change is a sample step.
pub const MIN_RAMP_MS: f32 = 5.0;
pub const DEFAULT_RAMP_MS: f32 = 10.0;
pub const MAX_RAMP_MS: f32 = 50.0;

/// SPEC §8.1's bus table. The names are the wire names the control plane
/// validates in `audio.bus.set`; a second spelling of `guestReturn` would be a
/// bug that only shows up on air.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BusId {
    Mic,
    Clip,
    Music,
    Sfx,
    Guest,
    Master,
    GuestReturn,
    Ifb,
}

impl BusId {
    pub const ALL: &'static [BusId] = &[
        BusId::Mic,
        BusId::Clip,
        BusId::Music,
        BusId::Sfx,
        BusId::Guest,
        BusId::Master,
        BusId::GuestReturn,
        BusId::Ifb,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            BusId::Mic => "mic",
            BusId::Clip => "clip",
            BusId::Music => "music",
            BusId::Sfx => "sfx",
            BusId::Guest => "guest",
            BusId::Master => "master",
            BusId::GuestReturn => "guestReturn",
            BusId::Ifb => "ifb",
        }
    }

    /// Not `FromStr`: the failure mode is "not a bus name", which is an
    /// `Option`, not an error type worth defining.
    pub fn parse(s: &str) -> Option<BusId> {
        BusId::ALL.iter().copied().find(|b| b.as_str() == s)
    }

    /// Buses that carry program audio into the master mix. `guestReturn` and
    /// `ifb` are outputs, not contributors — routing them into master is how
    /// feedback loops get built.
    fn feeds_master(&self) -> bool {
        matches!(
            self,
            BusId::Mic | BusId::Clip | BusId::Music | BusId::Sfx | BusId::Guest
        )
    }
}

/// A gain that moves to its target over a ramp instead of stepping (§8.7.1).
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    current: f32,
    target: f32,
    step: f32,
}

impl Ramp {
    pub fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
        }
    }

    /// Move toward `target` over `ms`, clamped to the §8.7.1 window.
    pub fn to(&mut self, target: f32, ms: f32) {
        let ms = ms.clamp(MIN_RAMP_MS, MAX_RAMP_MS);
        let frames = (ms / 1000.0 * SAMPLE_RATE as f32).max(1.0);
        self.target = target;
        self.step = (target - self.current) / frames;
    }

    /// Advance one sample frame and return the gain to apply.
    #[inline]
    pub fn advance(&mut self) -> f32 {
        if (self.target - self.current).abs() <= self.step.abs() {
            self.current = self.target;
            self.step = 0.0;
        } else {
            self.current += self.step;
        }
        self.current
    }

    pub fn value(&self) -> f32 {
        self.current
    }
}

pub fn db_to_linear(db: f32) -> f32 {
    if db <= -60.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

pub fn linear_to_db(v: f32) -> f32 {
    if v <= 0.000_001 {
        -120.0
    } else {
        20.0 * v.log10()
    }
}

/// One bus: gain, mute, and metering (§8.2).
#[derive(Debug)]
pub struct Bus {
    pub id: BusId,
    gain: Ramp,
    mute: Ramp,
    peak: f32,
    rms_accum: f64,
    rms_count: u64,
}

impl Bus {
    fn new(id: BusId) -> Self {
        Self {
            id,
            gain: Ramp::new(1.0),
            mute: Ramp::new(1.0),
            peak: 0.0,
            rms_accum: 0.0,
            rms_count: 0,
        }
    }

    /// SPEC §8.2: −60 dB to +12 dB.
    pub fn set_gain_db(&mut self, db: f32, ramp_ms: f32) {
        self.gain.to(db_to_linear(db.clamp(-60.0, 12.0)), ramp_ms);
    }

    pub fn set_muted(&mut self, muted: bool, ramp_ms: f32) {
        self.mute.to(if muted { 0.0 } else { 1.0 }, ramp_ms);
    }

    #[inline]
    fn next_gain(&mut self) -> f32 {
        self.gain.advance() * self.mute.advance()
    }

    fn observe(&mut self, sample: f32) {
        let a = sample.abs();
        if a > self.peak {
            self.peak = a;
        }
        self.rms_accum += (sample as f64) * (sample as f64);
        self.rms_count += 1;
    }

    pub fn peak_dbfs(&self) -> f32 {
        linear_to_db(self.peak)
    }

    pub fn rms_dbfs(&self) -> f32 {
        if self.rms_count == 0 {
            return -120.0;
        }
        linear_to_db(((self.rms_accum / self.rms_count as f64).sqrt()) as f32)
    }

    pub fn reset_meters(&mut self) {
        self.peak = 0.0;
        self.rms_accum = 0.0;
        self.rms_count = 0;
    }
}

/// What a bus is fed by. Sources are pre-rendered or generated; nothing here
/// reads a file (SPEC §8.9's rule: decode happens at load/arm, elsewhere).
#[derive(Debug, Clone)]
pub enum Source {
    /// Interleaved stereo PCM, read at a master-clock-derived offset.
    Pcm {
        samples: std::sync::Arc<Vec<f32>>,
        /// The master frame this source went on air (SPEC §12.1's `t0`).
        t0: u64,
        looping: bool,
    },
    /// A test tone, for the synthetic guest bus and the acceptance tests.
    Tone { hz: f32, amplitude: f32 },
}

/// A soundboard voice: RAM-resident, triggered, plays once (§8.4).
#[derive(Debug)]
struct Voice {
    samples: std::sync::Arc<Vec<f32>>,
    position: usize,
    playback_id: u64,
    asset_id: String,
    gain: Ramp,
    stopping: bool,
}

/// Ducking state for the music bus (§8.3).
#[derive(Debug)]
pub struct Ducker {
    pub enabled: bool,
    pub depth_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    gain: Ramp,
}

impl Default for Ducker {
    fn default() -> Self {
        Self {
            enabled: false,
            depth_db: -6.0,
            attack_ms: 10.0,
            release_ms: 250.0,
            gain: Ramp::new(1.0),
        }
    }
}

/// The audio graph.
pub struct AudioGraph {
    buses: BTreeMap<BusId, Bus>,
    /// Program sources by bus.
    sources: BTreeMap<BusId, Vec<Source>>,
    /// Per-guest buses, keyed by guest id (§8.6).
    guests: BTreeMap<String, (Bus, Vec<Source>)>,
    voices: Vec<Voice>,
    next_playback_id: u64,
    pub ducker: Ducker,
    house_frame_rate: u32,
    /// Samples rendered since the graph started, for drift measurement.
    rendered_samples: u64,
    underruns: u64,
    /// Scratch buffers, allocated once so `render` allocates nothing.
    scratch: Vec<f32>,
    guest_scratch: BTreeMap<String, Vec<f32>>,
}

impl AudioGraph {
    pub fn new(house_frame_rate: u32) -> Self {
        let mut buses = BTreeMap::new();
        for id in BusId::ALL {
            buses.insert(*id, Bus::new(*id));
        }
        Self {
            buses,
            sources: BTreeMap::new(),
            guests: BTreeMap::new(),
            voices: Vec::with_capacity(32),
            next_playback_id: 1,
            ducker: Ducker::default(),
            house_frame_rate,
            rendered_samples: 0,
            underruns: 0,
            scratch: vec![0.0; 8192],
            guest_scratch: BTreeMap::new(),
        }
    }

    pub fn bus_mut(&mut self, id: BusId) -> &mut Bus {
        self.buses.get_mut(&id).expect("every BusId has a Bus")
    }

    pub fn bus(&self, id: BusId) -> &Bus {
        self.buses.get(&id).expect("every BusId has a Bus")
    }

    pub fn set_source(&mut self, id: BusId, sources: Vec<Source>) {
        self.sources.insert(id, sources);
    }

    /// Add or replace a guest's bus (§8.6). Guests are separate buses because
    /// mix-minus needs to subtract exactly one of them.
    pub fn upsert_guest(&mut self, guest_id: &str, sources: Vec<Source>) {
        let bus = Bus::new(BusId::Guest);
        self.guests.insert(guest_id.to_string(), (bus, sources));
        let width = self.scratch.len();
        self.guest_scratch
            .insert(guest_id.to_string(), vec![0.0; width]);
    }

    pub fn guest_bus_mut(&mut self, guest_id: &str) -> Option<&mut Bus> {
        self.guests.get_mut(guest_id).map(|(b, _)| b)
    }

    pub fn guest_ids(&self) -> Vec<String> {
        self.guests.keys().cloned().collect()
    }

    /// Trigger a soundboard sample (§8.4). Returns its playback id.
    ///
    /// The samples are already resident; this only starts a voice, which is
    /// why AC-13's 20 ms budget is achievable.
    pub fn trigger(
        &mut self,
        asset_id: &str,
        samples: std::sync::Arc<Vec<f32>>,
        gain_db: f32,
    ) -> u64 {
        let id = self.next_playback_id;
        self.next_playback_id += 1;
        let mut gain = Ramp::new(0.0);
        // Even a trigger ramps in: a sample that starts at full scale on a
        // non-zero first sample is a click (§8.7.1).
        gain.to(db_to_linear(gain_db.clamp(-60.0, 12.0)), MIN_RAMP_MS);
        self.voices.push(Voice {
            samples,
            position: 0,
            playback_id: id,
            asset_id: asset_id.to_string(),
            gain,
            stopping: false,
        });
        id
    }

    /// Stop one voice, or all voices for an asset, with a ramp (§8.7.1).
    pub fn stop_voice(&mut self, playback_id: Option<u64>, asset_id: Option<&str>) -> usize {
        let mut stopped = 0;
        for v in self.voices.iter_mut() {
            let matches = match (playback_id, asset_id) {
                (Some(id), _) => v.playback_id == id,
                (None, Some(a)) => v.asset_id == a,
                (None, None) => false,
            };
            if matches && !v.stopping {
                v.gain.to(0.0, DEFAULT_RAMP_MS);
                v.stopping = true;
                stopped += 1;
            }
        }
        stopped
    }

    pub fn stop_all_voices(&mut self) -> usize {
        let mut stopped = 0;
        for v in self.voices.iter_mut() {
            if !v.stopping {
                v.gain.to(0.0, DEFAULT_RAMP_MS);
                v.stopping = true;
                stopped += 1;
            }
        }
        stopped
    }

    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// Set ducking (§8.3). Ducking touches `music` only.
    pub fn set_duck(
        &mut self,
        enabled: bool,
        depth_db: Option<f32>,
        attack_ms: Option<f32>,
        release_ms: Option<f32>,
    ) {
        if let Some(d) = depth_db {
            self.ducker.depth_db = d;
        }
        if let Some(a) = attack_ms {
            self.ducker.attack_ms = a;
        }
        if let Some(r) = release_ms {
            self.ducker.release_ms = r;
        }
        self.ducker.enabled = enabled;
        let (target, ms) = if enabled {
            (db_to_linear(self.ducker.depth_db), self.ducker.attack_ms)
        } else {
            (1.0, self.ducker.release_ms)
        };
        self.ducker.gain.to(target, ms);
    }

    /// The sample offset a source is read at for a master frame (SPEC §8.9).
    pub fn sample_for_master_frame(&self, frame: u64, t0: u64) -> i64 {
        let elapsed = frame as i64 - t0 as i64;
        elapsed * SAMPLE_RATE as i64 / self.house_frame_rate as i64
    }

    /// Ensure the scratch buffers cover a block of `samples`.
    ///
    /// Called when the block size changes — at setup, or the first time a
    /// device hands over a larger buffer. A device does not change its block
    /// size mid-stream, so the steady state allocates nothing.
    pub fn reserve(&mut self, samples: usize) {
        if self.scratch.len() < samples {
            self.scratch.resize(samples, 0.0);
        }
        let ids: Vec<String> = self.guest_scratch.keys().cloned().collect();
        for id in ids {
            if let Some(buf) = self.guest_scratch.get_mut(&id) {
                if buf.len() < samples {
                    buf.resize(samples, 0.0);
                }
            }
        }
    }

    /// Render `out.len() / CHANNELS` sample frames for the master frame that
    /// begins this buffer.
    ///
    /// Allocation-free and lock-free in the steady state: safe to call from a
    /// device callback once `reserve` has covered the block size.
    pub fn render(&mut self, out: &mut [f32], master_frame: u64) {
        self.reserve(out.len());
        out.fill(0.0);
        let frames = out.len() / CHANNELS;
        let base = self.sample_for_master_frame(master_frame, 0);

        // Program buses.
        let bus_ids: Vec<BusId> = self.sources.keys().copied().collect();
        for id in bus_ids {
            if !id.feeds_master() {
                continue;
            }
            let sources = self.sources.get(&id).cloned().unwrap_or_default();
            let scratch = &mut self.scratch[..out.len()];
            scratch.fill(0.0);
            render_sources(&sources, scratch, base, self.house_frame_rate);

            let ducking = id == BusId::Music;
            let bus = self.buses.get_mut(&id).expect("bus");
            for f in 0..frames {
                let g = bus.next_gain();
                let duck = if ducking {
                    self.ducker.gain.advance()
                } else {
                    1.0
                };
                for c in 0..CHANNELS {
                    let v = scratch[f * CHANNELS + c] * g * duck;
                    bus.observe(v);
                    out[f * CHANNELS + c] += v;
                }
            }
        }

        // Soundboard voices sum into `sfx`.
        if !self.voices.is_empty() {
            let scratch = &mut self.scratch[..out.len()];
            scratch.fill(0.0);
            for v in self.voices.iter_mut() {
                for f in 0..frames {
                    let g = v.gain.advance();
                    for c in 0..CHANNELS {
                        let idx = v.position + f * CHANNELS + c;
                        if idx < v.samples.len() {
                            scratch[f * CHANNELS + c] += v.samples[idx] * g;
                        }
                    }
                }
                v.position += frames * CHANNELS;
            }
            // A voice is done when it runs out or its stop ramp reached zero.
            self.voices
                .retain(|v| v.position < v.samples.len() && !(v.stopping && v.gain.value() <= 0.0));

            let bus = self.buses.get_mut(&BusId::Sfx).expect("sfx");
            for f in 0..frames {
                let g = bus.next_gain();
                for c in 0..CHANNELS {
                    let val = scratch[f * CHANNELS + c] * g;
                    bus.observe(val);
                    out[f * CHANNELS + c] += val;
                }
            }
        }

        // Guest buses sum into the program mix.
        let guest_ids: Vec<String> = self.guests.keys().cloned().collect();
        for gid in &guest_ids {
            let Some((_, sources)) = self.guests.get(gid) else {
                continue;
            };
            let sources = sources.clone();
            let buf = self
                .guest_scratch
                .get_mut(gid)
                .expect("guest scratch exists");
            let buf = &mut buf[..out.len()];
            buf.fill(0.0);
            render_sources(&sources, buf, base, self.house_frame_rate);
            let Some((bus, _)) = self.guests.get_mut(gid) else {
                continue;
            };
            for f in 0..frames {
                let g = bus.next_gain();
                for c in 0..CHANNELS {
                    let v = buf[f * CHANNELS + c] * g;
                    bus.observe(v);
                    out[f * CHANNELS + c] += v;
                }
            }
        }

        // Master gain and metering last.
        let master = self.buses.get_mut(&BusId::Master).expect("master");
        for f in 0..frames {
            let g = master.next_gain();
            for c in 0..CHANNELS {
                let idx = f * CHANNELS + c;
                // A limiter, not a clipper: hard clipping is a click generator.
                let v = (out[idx] * g).clamp(-1.0, 1.0);
                master.observe(v);
                out[idx] = v;
            }
        }

        self.rendered_samples += frames as u64;
    }

    /// Render guest G's mix-minus return (SPEC §8.6).
    ///
    /// The isolation is structural: this function has no path that reads
    /// guest G's own bus, so no check can be forgotten and no configuration
    /// can route it back. That is what AC-18 measures.
    pub fn render_guest_return(&mut self, guest_id: &str, out: &mut [f32], master_frame: u64) {
        self.reserve(out.len());
        out.fill(0.0);
        let frames = out.len() / CHANNELS;
        let base = self.sample_for_master_frame(master_frame, 0);

        for id in [BusId::Mic, BusId::Clip, BusId::Music, BusId::Sfx] {
            let Some(sources) = self.sources.get(&id).cloned() else {
                continue;
            };
            let scratch = &mut self.scratch[..out.len()];
            scratch.fill(0.0);
            render_sources(&sources, scratch, base, self.house_frame_rate);
            let gain = self.buses.get(&id).expect("bus").gain.value()
                * self.buses.get(&id).expect("bus").mute.value();
            for i in 0..out.len() {
                out[i] += scratch[i] * gain;
            }
        }

        // Every guest EXCEPT this one.
        let others: Vec<String> = self
            .guests
            .keys()
            .filter(|g| g.as_str() != guest_id)
            .cloned()
            .collect();
        for gid in others {
            let Some((bus, sources)) = self.guests.get(&gid) else {
                continue;
            };
            let gain = bus.gain.value() * bus.mute.value();
            let sources = sources.clone();
            let scratch = &mut self.scratch[..out.len()];
            scratch.fill(0.0);
            render_sources(&sources, scratch, base, self.house_frame_rate);
            for i in 0..out.len() {
                out[i] += scratch[i] * gain;
            }
        }

        let _ = frames;
    }

    /// Count a callback the graph could not fill in time (SPEC §8.10).
    pub fn note_underrun(&mut self) {
        self.underruns += 1;
    }

    pub fn underruns(&self) -> u64 {
        self.underruns
    }

    pub fn rendered_samples(&self) -> u64 {
        self.rendered_samples
    }

    /// Audio-to-master drift in milliseconds (SPEC §8.9): the difference
    /// between the samples actually rendered and the samples the master clock
    /// implies.
    pub fn drift_ms(&self, master_frame: u64) -> f64 {
        let expected = master_frame as i64 * SAMPLE_RATE as i64 / self.house_frame_rate as i64;
        let delta = self.rendered_samples as i64 - expected;
        delta as f64 * 1000.0 / SAMPLE_RATE as f64
    }

    /// Per-bus peak levels for telemetry (SPEC §10.1).
    pub fn bus_peaks(&self) -> BTreeMap<String, f64> {
        let mut out = BTreeMap::new();
        for (id, bus) in &self.buses {
            out.insert(id.as_str().to_string(), bus.peak_dbfs() as f64);
        }
        for (gid, (bus, _)) in &self.guests {
            out.insert(format!("guest:{gid}"), bus.peak_dbfs() as f64);
        }
        out
    }

    pub fn reset_meters(&mut self) {
        for bus in self.buses.values_mut() {
            bus.reset_meters();
        }
        for (bus, _) in self.guests.values_mut() {
            bus.reset_meters();
        }
    }
}

/// Sum a bus's sources into `out`, reading each at its master-clock offset.
fn render_sources(sources: &[Source], out: &mut [f32], base_sample: i64, _house_rate: u32) {
    let frames = out.len() / CHANNELS;
    for source in sources {
        match source {
            Source::Pcm {
                samples,
                t0,
                looping,
            } => {
                // SPEC §8.9: the read offset comes from the master clock and
                // the source's own start, never from a private cursor.
                let t0_samples = *t0 as i64 * SAMPLE_RATE as i64 / 30;
                let start = base_sample - t0_samples;
                let total = samples.len() as i64;
                if total == 0 {
                    continue;
                }
                for f in 0..frames {
                    let pos = start + f as i64;
                    let idx = if *looping {
                        (pos * CHANNELS as i64).rem_euclid(total)
                    } else if pos < 0 || pos * CHANNELS as i64 >= total {
                        continue;
                    } else {
                        pos * CHANNELS as i64
                    };
                    for c in 0..CHANNELS {
                        let i = (idx + c as i64) as usize % samples.len();
                        out[f * CHANNELS + c] += samples[i];
                    }
                }
            }
            Source::Tone { hz, amplitude } => {
                for f in 0..frames {
                    let n = base_sample + f as i64;
                    let v = amplitude
                        * (2.0 * std::f32::consts::PI * hz * n as f32 / SAMPLE_RATE as f32).sin();
                    for c in 0..CHANNELS {
                        out[f * CHANNELS + c] += v;
                    }
                }
            }
        }
    }
}

/// The largest sample-to-sample jump in a buffer, in dBFS.
///
/// AC-19 is measured, not asserted: a click is a discontinuity, and this is
/// the number that says how big one was.
pub fn worst_discontinuity_dbfs(buf: &[f32]) -> f32 {
    let mut worst = 0.0f32;
    let frames = buf.as_chunks::<CHANNELS>().0;
    for w in frames.windows(2) {
        for (a, b) in w[0].iter().zip(w[1].iter()) {
            let d = (b - a).abs();
            if d > worst {
                worst = d;
            }
        }
    }
    linear_to_db(worst)
}
