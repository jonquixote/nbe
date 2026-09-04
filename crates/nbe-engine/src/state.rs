//! Engine-side state (Prompt 03): the render node's own record — clock,
//! fallback residency, the last applied directive boundary, and the resync
//! holding pen.

use crate::clock::{ClockState, MasterClock};
use nbe_protocol::EngineFrame;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
    /// The effective quality profile, set once by the Prompt 04 probe.
    pub quality_profile: std::sync::Mutex<Option<nbe_protocol::QualityProfile>>,
    /// What the engine is showing on its view bus: the taken item's reference.
    /// Directive handlers update it; the render loop reads it.
    pub view_item: std::sync::Mutex<Option<String>>,
    /// The armed preview item, for the preview bus.
    pub preview_item: std::sync::Mutex<Option<String>>,
    /// The indexed package: scenes, items, decoded images (Prompt 04).
    pub package: Mutex<Option<crate::scene::PackageIndex>>,
    /// Bumped on every successful `show.load` so the renderer knows to
    /// re-upload its texture cache at a load boundary, never per frame.
    pub package_generation: AtomicU64,
    /// The running View transition, if any (Prompt 04 Step 2).
    pub transition: Mutex<Option<crate::scene::Transition>>,
    /// View frames not submitted by their deadline (SPEC §10.2).
    pub dropped_frames_total: AtomicU64,
    /// Preview misses: logged, never counted as dropped frames.
    pub preview_missed: AtomicU64,
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
            view_item: std::sync::Mutex::new(None),
            preview_item: std::sync::Mutex::new(None),
            package: Mutex::new(None),
            package_generation: AtomicU64::new(0),
            transition: Mutex::new(None),
            dropped_frames_total: AtomicU64::new(0),
            preview_missed: AtomicU64::new(0),
            degradation_rung: AtomicU64::new(0),
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

/// What the directive handler needs to emit back to the control plane.
#[derive(Default)]
pub struct OutgoingQueue {
    inner: Mutex<VecDeque<EngineFrame>>,
}

impl OutgoingQueue {
    pub fn push(&self, frame: EngineFrame) {
        self.inner.lock().unwrap().push_back(frame);
    }

    pub fn drain(&self) -> Vec<EngineFrame> {
        let mut q = self.inner.lock().unwrap();
        q.drain(..).collect()
    }
}

pub type SharedEngineState = Arc<EngineState>;
pub type SharedOutgoing = Arc<OutgoingQueue>;
