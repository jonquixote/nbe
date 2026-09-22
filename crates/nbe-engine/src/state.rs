//! Engine-side state (Prompt 03): the render node's own record — clock,
//! fallback residency, the last applied directive boundary, and the resync
//! holding pen.

use crate::clock::{ClockState, MasterClock};
use nbe_protocol::EngineFrame;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::Notify;

/// The engine's shared mutable state. All writes go through handlers; readers
/// (telemetry, watchdog) see a coherent snapshot via atomics.
pub struct EngineState {
    /// The clock is locked because Rust's ownership doesn't allow &mut behind
    /// Arc; lock order is: clock before everything else.
    pub clock: Mutex<MasterClock>,
    /// last applied stateVersion (what the control plane is signaling about)
    pub last_applied_state_version: AtomicU64,
    /// package path loaded by show.load (used by fallback residency check)
    pub package_path: Mutex<Option<String>>,
    /// resident fallback slate (path + loaded bytes), loaded at show.load
    pub fallback: Mutex<Option<FallbackSlate>>,
    /// set when the fallback is what the engine is currently showing
    pub fallback_active: AtomicBool,
    /// engine start time (telemetry)
    pub started_at: Instant,
    /// The effective quality profile: probe capped by request. Written only
    /// by `publish_quality_profile`.
    pub quality_profile: std::sync::Mutex<Option<nbe_protocol::QualityProfile>>,
    /// What the hardware probe found (SPEC §10.5).
    pub probed_quality: std::sync::Mutex<Option<nbe_protocol::QualityProfile>>,
    /// The wgpu device `RenderLoop::new` opened, published so the directive
    /// path can probe the zero-copy chain with **the device wgpu selected**.
    ///
    /// ZERO-COPY Phase 3b, step 2. Same shape as `probed_quality` directly
    /// above, and for the same reason §10.1.1 gives for that one: a hardware
    /// capability is probed by the render node, published into `EngineState`,
    /// and read back out by whoever needs it. `record.start` runs on the
    /// directive path and holds no wgpu handles of its own; a handshake-time
    /// answer would have to guess the take's geometry and would go stale
    /// silently on device loss, which is the moment it matters
    /// (`docs/zero-copy-p3-design.md`, Q1).
    ///
    /// `None` on every headless test and until the render loop has run. A
    /// `None` here is not an error — it is the machine saying it has no device,
    /// and the selection table's answer for that is `CpuReadback` /
    /// `ProbeUnavailable`.
    pub render_device: std::sync::Mutex<Option<std::sync::Arc<wgpu::Device>>>,
    /// What the manifest asked for (SPEC §10.1.1).
    pub requested_quality: std::sync::Mutex<Option<nbe_protocol::QualityProfile>>,
    /// What the engine is showing on its view bus: the taken item's reference.
    /// Directive handlers update it; the render loop reads it.
    pub view_item: std::sync::Mutex<Option<String>>,
    /// The master frame `preview_item` was armed — the Preview bus's `t0`.
    /// Without it an armed clip previews as black once the master clock passes
    /// the clip's own length.
    pub preview_item_start_frame: AtomicU64,
    /// The master frame `view_item` went on air — SPEC §12.1's `t0`.
    ///
    /// Without this the compositor reads every clip at `frame` rather than
    /// `frame - t0`, so a take at master frame N starts the clip N frames in.
    pub view_item_start_frame: AtomicU64,
    /// The armed preview item, for the preview bus.
    pub preview_item: std::sync::Mutex<Option<String>>,
    /// The indexed package: scenes, items, decoded images (Prompt 04).
    pub package: Mutex<Option<crate::scene::PackageIndex>>,
    /// Bumped on every successful `show.load` so the renderer knows to
    /// re-upload its texture cache at a load boundary, never per frame.
    pub package_generation: AtomicU64,
    /// The running View transition, if any (Prompt 04 Step 2).
    pub transition: Mutex<Option<crate::scene::Transition>>,
    /// Decoded video assets for the loaded package (Prompt 05).
    pub video: Mutex<crate::video::VideoLibrary>,
    /// The decode-session pool; its counts are what telemetry reports.
    pub sessions: crate::video::SessionPool,
    /// View frames not submitted by their deadline (SPEC §10.2).
    pub dropped_frames_total: AtomicU64,
    /// Preview misses: logged, never counted as dropped frames.
    pub preview_missed: AtomicU64,
    /// Audio callbacks the graph could not fill in time (SPEC §8.10).
    pub audio_underruns_total: AtomicU64,
    /// Every decode failure seen at `show.load` (SPEC §5.9.3).
    ///
    /// `VideoLibrary::failures` recorded them and nothing read it — F2. A
    /// failure the manifest can attribute to a rundown Item becomes an
    /// `itemEvent: decodeError`; one it cannot had no destination at all and
    /// left only a `warn!` behind, which is the single place "logged, therefore
    /// not swallowed" rested on an ungated line — F1.
    pub decode_failures_total: AtomicU64,
    /// The subset of the above that no rundown Item references.
    ///
    /// Counted separately because it is the operator-visible gap: nothing on
    /// the §17.3 state machine will ever turn red for these, so the count is
    /// the only evidence they happened.
    pub unattributable_decode_failures_total: AtomicU64,
    /// Audio-to-master drift (SPEC §8.9), as `f64::to_bits` so the audio
    /// thread can publish it without a lock.
    pub audio_drift_ms_bits: AtomicU64,
    /// Per-bus peak levels for telemetry (SPEC §10.1).
    pub bus_peaks: Mutex<std::collections::BTreeMap<String, f64>>,
    /// Audio intents published by the directive path and drained by whoever
    /// owns the graph. The directive thread never touches the graph itself.
    pub audio_commands: Mutex<Vec<crate::audio_control::AudioCommand>>,
    /// Soundboard samples, resident from `show.load` (SPEC §8.4). RAM-resident
    /// is the requirement: a trigger that reads disk cannot meet AC-13.
    pub audio_assets: Mutex<std::collections::BTreeMap<String, Arc<Vec<f32>>>>,
    /// Item ref → the asset whose audio that item plays (SPEC §7.1 scenes,
    /// §8.7.3 takes). Built once at `show.load` by walking item → scene →
    /// elements → asset, so a take is a map lookup and never a graph walk.
    pub item_audio: Mutex<std::collections::BTreeMap<String, String>>,
    /// One overlay's on-air state (SPEC §7.10). Timelines key off the master
    /// clock: `anim_start` is the master frame the animation begins — the frame
    /// after the command lands, the same boundary discipline AC-17 imposes on a
    /// take — never a transition frame.
    pub overlays: Mutex<std::collections::BTreeMap<String, OverlayRuntime>>,
    /// Engine recording state (SPEC §16.14). WU8 flips `Idle -> Recording` on
    /// `record.start` once the [`RecordSession`](crate::record::RecordSession)
    /// opens, and back on `record.stop` / `show.stop` quiescence after the
    /// session finishes.
    pub record_state: Mutex<RecordState>,
    /// The open recording, if any. `Some` exactly while `record_state` is
    /// `Recording` via the directive path (tests may arm the pair directly,
    /// the WU2 seam). `record.stop` takes + finishes it (bounded wait for the
    /// record thread) before the ack; `show.stop` quiesces it the same way.
    pub record_session: Mutex<Option<crate::record::RecordSession>>,
    /// The take's shared audio tap (WU-pipe): published by `record.start`,
    /// attached to the live graph by the audio driver, drained by the record
    /// thread. `None` while Idle. Cleared on every stop path so the driver
    /// detaches; the thread keeps its own `Arc` for the tail drain.
    pub record_tap: Mutex<Option<Arc<crate::record::AudioTap>>>,
    /// Accumulated record-feed cost in milliseconds (loop-updated, off the
    /// render budget by construction; observable to tests, no wire/telemetry
    /// change).
    pub record_tap_ms: Mutex<f64>,
    /// The frame path the record tap selected, and why (ZERO-COPY Phase 2).
    /// `None` until a take has selected one — which is what the telemetry
    /// field's absence means on the wire, rather than a defaulted guess.
    pub record_tap_selection: Mutex<Option<crate::record::tap_path::Selection>>,
    /// Record frames skipped over budget or on a saturated handoff
    /// (loop-updated, observable to tests). `Arc` so the record thread can
    /// count its own sheds into the same counter — no skip is invisible.
    pub skipped_record_frames: Arc<AtomicU64>,
    /// The loaded package's record target (`show.outputs.record.directory`,
    /// resolved against the package root). The channel telemetry pump reads
    /// this to measure `recordSpaceMib`; `None` when no package is loaded or
    /// the package declares no record target.
    pub record_dir: Mutex<Option<std::path::PathBuf>>,
    /// Engine streaming state (SPEC §16.14). WU4 flips `Idle -> Live` on
    /// `stream.start` once the [`StreamSession`](crate::record::stream::StreamSession)
    /// opens, and back on `stream.stop` / `show.stop` quiescence after the
    /// session closes.
    pub stream_state: Mutex<StreamState>,
    /// The live stream, if any. `Some` exactly while `stream_state` is `Live`
    /// via the directive path. `stream.stop` closes it before the ack;
    /// `show.stop` quiesces it the same way.
    pub stream_session: Mutex<Option<crate::record::stream::StreamSession>>,
    /// The frame path the live stream selected, and why (ZERO-COPY Phase 2
    /// shape, record-mirrored). `None` until a stream has selected one. Not
    /// cleared at stop: the field reads as the path the LAST stream used.
    pub stream_tap_selection: Mutex<Option<crate::record::tap_path::Selection>>,
    /// Current degradation rung (SPEC §10.5), as `Rung as u64`.
    degradation_rung: AtomicU64,
}

pub struct FallbackSlate {
    pub path: std::path::PathBuf,
    pub bytes: Vec<u8>,
}

impl EngineState {
    pub fn new(house_rate: u32) -> Self {
        Self {
            clock: Mutex::new(MasterClock::new(house_rate)),
            last_applied_state_version: AtomicU64::new(0),
            package_path: Mutex::new(None),
            fallback: Mutex::new(None),
            fallback_active: AtomicBool::new(false),
            started_at: Instant::now(),
            quality_profile: std::sync::Mutex::new(None),
            probed_quality: std::sync::Mutex::new(None),
            render_device: std::sync::Mutex::new(None),
            requested_quality: std::sync::Mutex::new(None),
            view_item: std::sync::Mutex::new(None),
            view_item_start_frame: AtomicU64::new(0),
            preview_item_start_frame: AtomicU64::new(0),
            preview_item: std::sync::Mutex::new(None),
            package: Mutex::new(None),
            package_generation: AtomicU64::new(0),
            transition: Mutex::new(None),
            video: Mutex::new(crate::video::VideoLibrary::default()),
            sessions: crate::video::SessionPool::new(),
            dropped_frames_total: AtomicU64::new(0),
            preview_missed: AtomicU64::new(0),
            audio_underruns_total: AtomicU64::new(0),
            decode_failures_total: AtomicU64::new(0),
            unattributable_decode_failures_total: AtomicU64::new(0),
            audio_drift_ms_bits: AtomicU64::new(0),
            bus_peaks: Mutex::new(std::collections::BTreeMap::new()),
            audio_commands: Mutex::new(Vec::new()),
            audio_assets: Mutex::new(std::collections::BTreeMap::new()),
            item_audio: Mutex::new(std::collections::BTreeMap::new()),
            overlays: Mutex::new(std::collections::BTreeMap::new()),
            record_state: Mutex::new(RecordState::Idle),
            record_session: Mutex::new(None),
            record_tap: Mutex::new(None),
            record_tap_ms: Mutex::new(0.0),
            record_tap_selection: Mutex::new(None),
            skipped_record_frames: Arc::new(AtomicU64::new(0)),
            record_dir: Mutex::new(None),
            stream_state: Mutex::new(StreamState::Idle),
            stream_session: Mutex::new(None),
            stream_tap_selection: Mutex::new(None),
            degradation_rung: AtomicU64::new(0),
        }
    }

    /// Publish the effective quality profile: the probe result capped by the
    /// manifest's request (SPEC §10.1.1).
    ///
    /// Both inputs live here, and the cap is applied at publish time rather
    /// than at load time, so a later GPU re-init (device loss) that re-probes
    /// cannot silently drop the cap.
    pub fn publish_quality_profile(&self) {
        let probed = *self.probed_quality.lock().unwrap();
        let requested = *self.requested_quality.lock().unwrap();
        let effective = match (probed, requested) {
            (Some(p), Some(r)) => Some(p.capped_by(r)),
            (Some(p), None) => Some(p),
            _ => None,
        };
        *self.quality_profile.lock().unwrap() = effective;
    }

    /// Record what the hardware probe found, then republish.
    pub fn set_probed_quality(&self, probed: nbe_protocol::QualityProfile) {
        *self.probed_quality.lock().unwrap() = Some(probed);
        self.publish_quality_profile();
    }

    /// Publish the device the render loop opened.
    ///
    /// Publishing, not assigning: a re-init after device loss replaces the
    /// handle at the same point the quality probe is re-applied, so the two
    /// GPU-derived facts cannot drift apart.
    pub fn set_render_device(&self, device: std::sync::Arc<wgpu::Device>) {
        *self.render_device.lock().unwrap() = Some(device);
    }

    /// The device the render loop opened, if one has been published.
    ///
    /// Returns a clone of the `Arc` rather than lending the guard: the probe
    /// this feeds builds an IOSurface and a Metal texture, and holding a
    /// `MutexGuard` across that would put GPU work inside a lock the render
    /// loop also takes.
    pub fn render_device(&self) -> Option<std::sync::Arc<wgpu::Device>> {
        self.render_device.lock().unwrap().clone()
    }

    /// Record what the manifest asked for, then republish.
    pub fn set_requested_quality(&self, requested: Option<nbe_protocol::QualityProfile>) {
        *self.requested_quality.lock().unwrap() = requested;
        self.publish_quality_profile();
    }

    /// One consistent read of everything the renderer needs for a frame.
    ///
    /// Taken in one place so the View and Preview cannot disagree about which
    /// item is on air, and so decode threads writing to this state cannot be
    /// observed halfway.
    pub fn frame_snapshot(&self) -> FrameSnapshot {
        FrameSnapshot {
            view_item: self.view_item.lock().unwrap().clone(),
            view_item_start_frame: self.view_item_start_frame.load(Ordering::SeqCst),
            preview_item_start_frame: self.preview_item_start_frame.load(Ordering::SeqCst),
            preview_item: self.preview_item.lock().unwrap().clone(),
            transition: self.transition.lock().unwrap().clone(),
            fallback_active: self.fallback_active.load(Ordering::SeqCst),
        }
    }

    pub fn rung(&self) -> crate::render::Rung {
        match self.degradation_rung.load(Ordering::SeqCst) {
            0 => crate::render::Rung::Nominal,
            _ => crate::render::Rung::PreviewHalfRate,
        }
    }

    pub fn set_rung(&self, rung: crate::render::Rung) {
        self.degradation_rung.store(rung as u64, Ordering::SeqCst);
    }

    pub fn degradation_rung(&self) -> u32 {
        self.degradation_rung.load(Ordering::SeqCst) as u32
    }

    /// The slate the View shows when the fallback engages. Decoded from the
    /// resident bytes Prompt 03 loaded; an asset that will not decode yields a
    /// generated slate, because "no picture" is not an option on air
    /// (SPEC §7.14).
    pub fn fallback_image(&self) -> crate::scene::DecodedImage {
        let guard = self.fallback.lock().unwrap();
        let decoded = guard
            .as_ref()
            .and_then(|f| crate::scene::DecodedImage::decode(&f.bytes));
        match decoded {
            Some(img) => img,
            None => {
                if guard.is_some() {
                    tracing::warn!(
                        "fallback asset is not a decodable image; using a generated slate"
                    );
                }
                crate::scene::DecodedImage::generated_slate(
                    crate::render::VIEW_W,
                    crate::render::VIEW_H,
                    [191, 89, 13, 255],
                )
            }
        }
    }

    /// The house rate this show runs at. The renderer needs it to map show
    /// time onto source time for non-house-rate assets (SPEC §18).
    pub fn house_rate(&self) -> u32 {
        self.clock.lock().unwrap().house_rate()
    }

    pub fn clock_state(&self) -> ClockState {
        self.clock.lock().unwrap().state()
    }

    pub fn is_running(&self) -> bool {
        self.clock.lock().unwrap().state() == ClockState::Running
    }

    pub fn master_frame(&self) -> Option<u64> {
        self.clock.lock().unwrap().frame()
    }

    /// Seconds since engine boot — used by telemetry so a control plane can
    /// detect an engine that stopped reporting (engineConnected staleness).
    pub fn uptime_secs(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64()
    }

    pub fn set_last_applied(&self, v: u64) {
        self.last_applied_state_version.store(v, Ordering::SeqCst);
    }

    pub fn last_applied(&self) -> u64 {
        self.last_applied_state_version.load(Ordering::SeqCst)
    }
}

/// One frame's worth of show state, read once (Prompt 05 Step 8).
#[derive(Debug, Clone)]
pub struct FrameSnapshot {
    pub view_item: Option<String>,
    /// SPEC §12.1's `t0` for the View bus item.
    pub view_item_start_frame: u64,
    /// The Preview bus's `t0`.
    pub preview_item_start_frame: u64,
    pub preview_item: Option<String>,
    pub transition: Option<crate::scene::Transition>,
    pub fallback_active: bool,
}

/// What the directive handler needs to emit back to the control plane.
#[derive(Default)]
pub struct OutgoingQueue {
    inner: Mutex<VecDeque<EngineFrame>>,
    /// Wakes the outbound pump the moment a frame is queued.
    ///
    /// Without it the pump only looked at this queue once per
    /// `telemetry_interval_ms`, so every engine frame — including §5.9.5's
    /// `appliedStateVersion` acknowledgements — was quantised to 1 Hz. The
    /// acks were never missing; they were up to a second late, and a
    /// `stateChange` frame snapshots `renderNode` at command-accept time, which
    /// is always before a second has passed. That is the whole of "R6:
    /// appliedStateVersion freezes after resync".
    ready: Notify,
}

impl OutgoingQueue {
    pub fn push(&self, frame: EngineFrame) {
        self.inner.lock().unwrap().push_back(frame);
        self.ready.notify_one();
    }

    pub fn drain(&self) -> Vec<EngineFrame> {
        let mut q = self.inner.lock().unwrap();
        q.drain(..).collect()
    }

    /// Resolves when a frame has been queued since the last drain.
    ///
    /// `Notify::notify_one` stores one permit, so a push that happens between
    /// a drain and this await still wakes it — no frame waits for the next one.
    pub async fn ready(&self) {
        self.ready.notified().await;
    }
}

pub type SharedEngineState = Arc<EngineState>;
pub type SharedOutgoing = Arc<OutgoingQueue>;

/// One overlay's on-air state (SPEC §7.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayRuntime {
    pub on_air: bool,
    /// Master frame the current animation began on (the frame after the show/
    /// hide command landed).
    pub anim_start: u64,
    /// Length of the current animation in frames.
    pub duration_frames: u64,
    /// Which direction the current animation travels.
    pub phase: OverlayPhase,
}

/// What the overlay's current animation is doing, as a master-clock function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayPhase {
    /// animating in (opacity 0 → 1).
    Enter,
    /// fully on air (opacity 1).
    Steady,
    /// animating out (opacity 1 → 0); the overlay drops when it completes.
    Exit,
}

/// Engine recording state (SPEC §16.14): `Idle → Recording → (stop) Idle`.
/// WU8 enters `Recording` on `record.start` (encoder available + record target
/// configured); `record.stop` and `show.stop` quiescence return it to `Idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordState {
    #[default]
    Idle,
    Recording,
}

/// Engine streaming state (SPEC §16.14): `Idle → Live → (stop) Idle`.
/// WU4 enters `Live` on `stream.start` (show running + encoder available +
/// zero-copy chain available + endpoint resolved); `stream.stop` and
/// `show.stop` quiescence return it to `Idle`. Exactly one live stream exists
/// at a time (§9.1 ceiling — a second start while `Live` is refused).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamState {
    #[default]
    Idle,
    Live,
}
