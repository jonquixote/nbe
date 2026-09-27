//! Prompt 11, B1 (SPEC v0.4.7 candidate): the engine's `audioLevelCrossing`
//! event.
//!
//! An `audioLevel` rule must fire within one frame of its bus crossing the
//! threshold (AC-25 #1). The §10.1 tick carries `busPeakDbfs` once a second,
//! so the control plane cannot see a crossing in time; the engine computes it
//! on the audio block (one block per house frame) and sends it on the render
//! channel, and the control plane's evaluator matches it to rules.
//!
//! Every test here meters REAL levels: a tone source on a graph bus, measured
//! by the graph's own peak meter, compared against the watch in
//! `AudioDriver::publish`. Nothing hands the driver a level. The load path is
//! entered through `DirectiveHandler::apply("show.load")` (rule 7): the
//! watches come from the manifest's automation rules the way production gets
//! them. All tests are hardware-free.

use std::sync::Arc;

use nbe_core::automation::{AudioLevelWatch, CrossingDirection as CoreDirection, AUDIO_BUSES};
use nbe_engine::audio::{BusId, Source, SAMPLE_RATE};
use nbe_engine::audio_driver::{AudioDriver, NullSink};
use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{
    CrossingDirection, DirectiveFrame, DirectiveKind, EngineFrame, ItemEvent, PROTOCOL_VERSION,
};

const HOUSE_RATE: u32 = 30;
const BLOCK: usize = SAMPLE_RATE as usize / HOUSE_RATE as usize;

fn driver(state: Arc<EngineState>, outgoing: Arc<OutgoingQueue>) -> AudioDriver {
    AudioDriver::new(state, Box::new(NullSink::new(BLOCK)), HOUSE_RATE).with_events(outgoing)
}

fn watch(bus: &str, threshold: f64, direction: CoreDirection) -> AudioLevelWatch {
    AudioLevelWatch {
        bus: bus.into(),
        threshold_dbfs: threshold,
        direction,
    }
}

fn install(state: &EngineState, watches: Vec<AudioLevelWatch>) {
    *state.audio_level_watches.lock().unwrap() = Arc::new(watches);
}

fn tone(driver: &mut AudioDriver, bus: BusId, amplitude: f32) {
    driver.graph.set_source(
        bus,
        vec![Source::Tone {
            hz: 1000.0,
            amplitude,
        }],
    );
}

fn silence(driver: &mut AudioDriver, bus: BusId) {
    driver.graph.set_source(bus, Vec::new());
}

/// Every `audioLevelCrossing` frame queued so far, as
/// `(bus, threshold, direction, level, masterFrame)`.
fn crossings(outgoing: &OutgoingQueue) -> Vec<(String, f64, CrossingDirection, f64, u64)> {
    outgoing
        .drain()
        .into_iter()
        .filter_map(|f| match f {
            EngineFrame::AudioLevelCrossing {
                bus,
                threshold_dbfs,
                direction,
                level_dbfs,
                master_frame,
                ..
            } => Some((bus, threshold_dbfs, direction, level_dbfs, master_frame)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_rising_crossing_is_reported_on_the_block_it_happens_in() {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let outgoing = Arc::new(OutgoingQueue::default());
    let mut d = driver(state.clone(), outgoing.clone());
    install(&state, vec![watch("mic", -12.0, CoreDirection::Rising)]);

    // Silent block: below the threshold, nothing to report.
    d.cycle(10);
    assert!(crossings(&outgoing).is_empty(), "silence crosses nothing");

    // A tone at ~-0.9 dBFS on the mic bus: the NEXT block crosses, and the
    // frame names that block's master frame.
    tone(&mut d, BusId::Mic, 0.9);
    d.cycle(11);
    let got = crossings(&outgoing);
    assert_eq!(
        got.len(),
        1,
        "exactly one crossing, on the block it happened: {got:?}"
    );
    let (bus, threshold, direction, level, frame) = got[0].clone();
    assert_eq!(bus, "mic");
    assert_eq!(threshold, -12.0);
    assert_eq!(direction, CrossingDirection::Rising);
    assert!(
        level > -12.0 && level <= 0.0,
        "the measured level is reported: {level}"
    );
    assert_eq!(frame, 11, "the crossing names its own block's master frame");

    // Still above: a level that stays up does not cross again.
    d.cycle(12);
    d.cycle(13);
    assert!(
        crossings(&outgoing).is_empty(),
        "staying above is not a crossing"
    );
    assert_eq!(d.crossings(), 1);
}

#[test]
fn a_falling_crossing_fires_when_the_level_drops_below() {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let outgoing = Arc::new(OutgoingQueue::default());
    let mut d = driver(state.clone(), outgoing.clone());
    install(&state, vec![watch("music", -30.0, CoreDirection::Falling)]);

    tone(&mut d, BusId::Music, 0.9);
    d.cycle(0);
    d.cycle(1);
    assert!(
        crossings(&outgoing).is_empty(),
        "a falling watch ignores the rise"
    );
    silence(&mut d, BusId::Music);
    d.cycle(2);
    let got = crossings(&outgoing);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].2, CrossingDirection::Falling);
    assert!(
        got[0].3 < -30.0,
        "the level that fell is reported: {}",
        got[0].3
    );
    assert_eq!(got[0].4, 2);
}

/// Stub-proofing: the comparison is against the graph's MEASURED level. A
/// tone at ~-20 dBFS does not cross a -12 threshold; the same bus at ~-0.9
/// does. A driver that reported crossings without measuring could not tell
/// these apart.
#[test]
fn the_threshold_is_compared_with_the_measured_level() {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let outgoing = Arc::new(OutgoingQueue::default());
    let mut d = driver(state.clone(), outgoing.clone());
    install(&state, vec![watch("mic", -12.0, CoreDirection::Rising)]);

    tone(&mut d, BusId::Mic, 0.1); // 20·log10(0.1) = -20 dBFS
    for f in 0..5 {
        d.cycle(f);
    }
    assert!(
        crossings(&outgoing).is_empty(),
        "-20 dBFS is below a -12 threshold and must not cross"
    );
    tone(&mut d, BusId::Mic, 0.9);
    d.cycle(5);
    assert_eq!(crossings(&outgoing).len(), 1, "-0.9 dBFS crosses -12");
}

/// Rule 7, from the command to the frame: `show.load` of a package whose
/// manifest carries automation rules installs exactly the valid
/// `audioLevel` watches — a malformed one is skipped (preflight refuses it
/// before load), other trigger kinds are not engine watches — and a real
/// level on that bus then crosses the manifest's own threshold.
#[tokio::test]
async fn show_load_installs_the_rules_watches_and_a_real_level_crosses_them() {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    let (_pkg, pkg_path) = write_package(serde_json::json!([
        { "id": "hot-mic", "trigger": { "kind": "audioLevel",
            "params": { "bus": "mic", "thresholdDbfs": -18 } },
          "action": { "command": "marker.add", "payload": { "name": "hot" } } },
        { "id": "typo", "trigger": { "kind": "audioLevel",
            "params": { "bus": "mic", "treshold": -18 } },
          "action": { "command": "marker.add", "payload": { "name": "x" } } },
        { "id": "tick", "trigger": { "kind": "timer", "params": { "atMs": 1000 } },
          "action": { "command": "marker.add", "payload": { "name": "t" } } }
    ]));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": pkg_path.to_string_lossy() }),
        ))
        .await
        .expect("show.load of the test package must succeed");
    assert_eq!(
        **state.audio_level_watches.lock().unwrap(),
        vec![watch("mic", -18.0, CoreDirection::Rising)],
        "exactly the valid audioLevel rule becomes a watch"
    );
    outgoing.drain(); // the load's own ack

    let mut d = driver(state.clone(), outgoing.clone());
    d.cycle(0);
    tone(&mut d, BusId::Mic, 0.9);
    d.cycle(1);
    let got = crossings(&outgoing);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!((got[0].0.as_str(), got[0].1), ("mic", -18.0));
}

/// A crossing is a moment: queued while no control plane listened, it is
/// dropped when one connects (`RenderChannel::run` calls this first), while
/// acks and item events — states that are still true — are kept.
#[test]
fn crossings_queued_during_an_outage_are_not_replayed() {
    let q = OutgoingQueue::default();
    q.push(EngineFrame::AppliedStateVersion {
        v: PROTOCOL_VERSION.into(),
        state_version: 7,
    });
    q.push(EngineFrame::AudioLevelCrossing {
        v: PROTOCOL_VERSION.into(),
        bus: "mic".into(),
        threshold_dbfs: -12.0,
        direction: CrossingDirection::Rising,
        level_dbfs: -3.0,
        master_frame: 99,
    });
    q.push(EngineFrame::ItemEvent {
        v: PROTOCOL_VERSION.into(),
        item_ref: "A1".into(),
        event: ItemEvent::End,
        detail: None,
    });
    assert_eq!(q.discard_stale_crossings(), 1);
    let kept: Vec<_> = q.drain();
    assert_eq!(kept.len(), 2);
    assert!(kept
        .iter()
        .all(|f| !matches!(f, EngineFrame::AudioLevelCrossing { .. })));
}

/// The params contract's bus list (`nbe_core::automation::AUDIO_BUSES`) is
/// the graph's own: a name preflight accepts is a bus the engine meters, and
/// no metered bus is refused.
#[test]
fn the_bus_names_match_the_engine_graph() {
    let graph: Vec<&str> = BusId::ALL.iter().map(|b| b.as_str()).collect();
    assert_eq!(
        graph, AUDIO_BUSES,
        "nbe-core's bus list drifted from audio::BusId"
    );
}

// ---------------------------------------------------------------------------

fn directive(command: &str, sv: u64, payload: serde_json::Value) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target: serde_json::json!({}),
        payload,
    }
}

/// A minimal loadable package carrying `automation`.
fn write_package(automation: serde_json::Value) -> (tempfile::TempDir, std::path::PathBuf) {
    let pkg = tempfile::tempdir().expect("package tempdir must succeed");
    std::fs::create_dir_all(pkg.path().join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(pkg.path().join("media/slate.png"), &png).unwrap();
    std::fs::write(
        pkg.path().join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.4",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" }
            ],
            "scenes": [],
            "rundown": { "id": "R", "items": [] },
            "control": { "bindings": [] },
            "automation": automation
        })
        .to_string(),
    )
    .unwrap();
    let pkg_path = pkg.path().to_path_buf();
    (pkg, pkg_path)
}
