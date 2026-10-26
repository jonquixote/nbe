//! Audio intents: the bridge from the directive path to the audio graph
//! (Prompt 06 Step 2).
//!
//! The directive thread parses a frame into one of these and publishes it. The
//! owner of the graph applies them. Nothing here touches the graph, and nothing
//! here allocates on the audio thread's side of the boundary — SPEC §8.9's
//! rule is that control crosses as data, not as a call.

use crate::audio::{db_to_linear, AudioGraph, BusId, DEFAULT_RAMP_MS};
use crate::directive::DirectiveError;
use nbe_protocol::DirectiveFrame;

/// One audio intent, parsed from a Section 16.8 directive.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioCommand {
    Play {
        asset_id: String,
        gain_db: f32,
    },
    Stop {
        playback_id: Option<u64>,
        asset_id: Option<String>,
    },
    StopAll,
    BusSet {
        bus: String,
        guest_id: Option<String>,
        gain_db: Option<f32>,
        muted: Option<bool>,
    },
    Duck {
        enabled: bool,
        depth_db: Option<f32>,
        attack_ms: Option<f32>,
        release_ms: Option<f32>,
    },
    GuestMute {
        guest_id: String,
        muted: bool,
    },
    /// The master limiter's ceiling for the loaded show (SPEC §8.2's note):
    /// its `truePeakDbtp`, or the spec default when it declares none.
    SetLimiterCeiling {
        dbtp: f64,
        /// `"show"` or `"default"`, for the log line that records it.
        source: &'static str,
    },
    /// A take: put this item's audio on its buses at its `t0`, honouring the
    /// §8.7.3 audio mode.
    TakeItem {
        item_ref: String,
        /// What this item plays, one source per audible element of its scene
        /// (D4), resolved at load time from the package index. Empty means
        /// the item shows nothing audible — a graphic scene, legitimately
        /// silent.
        sources: Vec<crate::scene::ElementAudio>,
        t0: u64,
        /// `follow` | `crossfade` | `cut` | `mute` (SPEC §8.7.3).
        mode: String,
        ramp_ms: f32,
        /// Frames to crossfade over; 0 for a cut (SPEC §8.7.5/§8.7.6).
        crossfade_frames: u64,
    },
}

impl AudioCommand {
    pub fn from_directive(d: &DirectiveFrame) -> Result<Self, DirectiveError> {
        let p = &d.payload;
        let f32_of = |k: &str| p.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
        let str_of = |k: &str| p.get(k).and_then(|v| v.as_str()).map(str::to_string);
        Ok(match d.command.as_str() {
            "soundboard.play" => AudioCommand::Play {
                asset_id: str_of("assetId").ok_or_else(|| {
                    DirectiveError::Invalid("soundboard.play needs assetId".into())
                })?,
                gain_db: f32_of("gainDb").unwrap_or(0.0),
            },
            "soundboard.stop" => AudioCommand::Stop {
                playback_id: p.get("playbackId").and_then(|v| v.as_u64()),
                asset_id: str_of("assetId"),
            },
            "soundboard.stopAll" => AudioCommand::StopAll,
            "audio.bus.set" => AudioCommand::BusSet {
                bus: str_of("bus")
                    .ok_or_else(|| DirectiveError::Invalid("audio.bus.set needs bus".into()))?,
                guest_id: str_of("guestId"),
                gain_db: f32_of("gainDb"),
                muted: p.get("muted").and_then(|v| v.as_bool()),
            },
            "audio.duck" => AudioCommand::Duck {
                enabled: p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
                depth_db: f32_of("depthDb"),
                attack_ms: f32_of("attackMs"),
                release_ms: f32_of("releaseMs"),
            },
            "guest.mute" => AudioCommand::GuestMute {
                guest_id: str_of("guestId")
                    .ok_or_else(|| DirectiveError::Invalid("guest.mute needs guestId".into()))?,
                muted: p.get("muted").and_then(|v| v.as_bool()).unwrap_or(false),
            },
            other => {
                return Err(DirectiveError::Invalid(format!(
                    "{other} is not an audio directive"
                )))
            }
        })
    }
}

/// The buses an element's `audio.bus` can name (`LayerAudio.bus`), each fed
/// by the take (D4).
pub const ELEMENT_BUSES: [BusId; 5] = [
    BusId::Clip,
    BusId::Music,
    BusId::Sfx,
    BusId::Guest,
    BusId::Mic,
];

/// Apply intents to the graph. Called by the graph's owner, never by the
/// directive thread.
///
/// `library` supplies soundboard samples that were preloaded at show load —
/// nothing is read from disk here (SPEC §8.9).
/// How a take's audio mode maps to the clip bus (SPEC §8.7.3).
///
/// Returned rather than applied inline so the mapping is testable without a
/// graph: it is the part of the take that is easy to get subtly wrong.
pub fn take_gain_and_ramp(
    mode: &str,
    ramp_ms: f32,
    crossfade_frames: u64,
    house_rate: u32,
) -> (f32, f32) {
    let crossfade_ms = if crossfade_frames > 0 {
        crossfade_frames as f32 * 1000.0 / house_rate.max(1) as f32
    } else {
        0.0
    };
    match mode {
        // The item's audio is muted on air.
        "mute" => (-60.0, ramp_ms),
        // A linear crossfade over the video's own duration (§8.7.5): `Ramp`
        // advances in linear gain steps (`Ramp::advance` adds a fixed step
        // per sample frame toward the db_to_linear target) — spec-legal,
        // not equal-power. The ramp is the crossfade; the clip arrives at
        // unity across it.
        "crossfade" => (0.0, crossfade_ms.max(ramp_ms)),
        // A cut still ramps: §8.7.6 sets a 5 ms floor, which `Ramp` enforces.
        "cut" => (0.0, ramp_ms),
        // `follow` is AFV — the item's audioPolicy decides, and `clip` is the
        // default policy, so the clip bus comes up at unity.
        _ => (
            0.0,
            if crossfade_ms > 0.0 {
                crossfade_ms
            } else {
                ramp_ms
            },
        ),
    }
}

pub fn apply(
    graph: &mut AudioGraph,
    commands: Vec<AudioCommand>,
    library: &std::collections::BTreeMap<String, std::sync::Arc<Vec<f32>>>,
) {
    for cmd in commands {
        match cmd {
            AudioCommand::Play { asset_id, gain_db } => match library.get(&asset_id) {
                Some(samples) => {
                    graph.trigger(&asset_id, samples.clone(), gain_db);
                }
                None => tracing::warn!(asset = %asset_id, "soundboard asset not resident"),
            },
            AudioCommand::Stop {
                playback_id,
                asset_id,
            } => {
                graph.stop_voice(playback_id, asset_id.as_deref());
            }
            AudioCommand::StopAll => {
                graph.stop_all_voices();
            }
            AudioCommand::BusSet {
                bus,
                guest_id,
                gain_db,
                muted,
            } => {
                // SPEC §16.8: guestId is required for guestReturn and ignored
                // otherwise. The graph honours that rather than reinterpreting.
                if bus == "guestReturn" {
                    if let Some(gid) = guest_id.as_deref() {
                        if let Some(b) = graph.guest_bus_mut(gid) {
                            if let Some(db) = gain_db {
                                b.set_gain_db(db, DEFAULT_RAMP_MS);
                            }
                            if let Some(m) = muted {
                                b.set_muted(m, DEFAULT_RAMP_MS);
                            }
                        }
                    }
                    continue;
                }
                let Some(id) = BusId::parse(&bus) else {
                    tracing::warn!(bus = %bus, "unknown audio bus");
                    continue;
                };
                let b = graph.bus_mut(id);
                if let Some(db) = gain_db {
                    b.set_operator_gain_db(db, DEFAULT_RAMP_MS);
                }
                if let Some(m) = muted {
                    b.set_muted(m, DEFAULT_RAMP_MS);
                }
            }
            AudioCommand::Duck {
                enabled,
                depth_db,
                attack_ms,
                release_ms,
            } => graph.set_duck(enabled, depth_db, attack_ms, release_ms),
            AudioCommand::TakeItem {
                item_ref,
                sources,
                t0,
                mode,
                ramp_ms,
                crossfade_frames,
            } => {
                let house = graph.house_frame_rate();
                let (gain_db, ramp) = take_gain_and_ramp(&mode, ramp_ms, crossfade_frames, house);
                // Every element bus is swapped through silence (§8.7.1): the
                // content changes, not just the level, so ramping the gain
                // alone would still step the waveform. Each source is resolved
                // by ASSET id, and only by asset id (the P0: the library is
                // keyed by asset, and resolving by `item_ref` took silently).
                for e in &sources {
                    if BusId::parse(&e.bus).is_none_or(|b| !ELEMENT_BUSES.contains(&b)) {
                        tracing::warn!(item = %item_ref, element = %e.element_id, bus = %e.bus,
                            "element audio names no programme bus; not played");
                    }
                }
                for bus in ELEMENT_BUSES {
                    let feed: Vec<crate::audio::Source> = sources
                        .iter()
                        .filter(|e| BusId::parse(&e.bus) == Some(bus))
                        .filter_map(|e| {
                            library
                                .get(&e.asset_id)
                                .map(|samples| crate::audio::Source::Pcm {
                                    samples: samples.clone(),
                                    t0,
                                    looping: e.looping,
                                    gain: db_to_linear(e.gain_db),
                                })
                        })
                        .collect();
                    if bus == BusId::Clip {
                        // The clip bus takes the item's audio mode, as it
                        // always has, and a silent item clears it rather than
                        // leaving the previous item audible under the new
                        // picture.
                        graph.swap_source_through_silence(bus, feed, gain_db, ramp);
                    } else if !feed.is_empty() || graph.has_sources(bus) {
                        // Another element bus returns to the operator's level
                        // (`audio.bus.set`), muted only by a `mute` take: a take
                        // must not reset what the operator set on music or sfx.
                        let level = if mode == "mute" {
                            -60.0
                        } else {
                            graph.bus(bus).operator_gain_db()
                        };
                        graph.swap_source_through_silence(bus, feed, level, ramp);
                    }
                }
            }
            AudioCommand::SetLimiterCeiling { dbtp, source } => {
                graph.set_limiter_ceiling_dbtp(dbtp);
                // The ceiling in force is recorded (the user's ruling L1).
                tracing::info!(
                    ceiling_dbtp = dbtp,
                    source,
                    latency_frames = graph.latency_frames(),
                    "master limiter ceiling in force"
                );
            }
            AudioCommand::GuestMute { guest_id, muted } => {
                if let Some(b) = graph.guest_bus_mut(&guest_id) {
                    b.set_muted(muted, DEFAULT_RAMP_MS);
                }
            }
        }
    }
}
