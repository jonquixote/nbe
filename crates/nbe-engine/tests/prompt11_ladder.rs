//! Prompt 11 WU6 — degradation ladder rung 2, "loop caches evict to
//! streaming" (SPEC §10.5, AC-27 item 2), built as the user decided on
//! 2026-09-27: eviction applies ONLY to loops not on air.
//!
//! The invariant is §10.5's own clause — "the View MUST NOT be degraded" — so
//! the loop feeding the View is never evicted, and no eviction can freeze it.
//! Off-air loops (preview, prerolled) shed their VRAM ring under rung 2 and are
//! re-acquired on the next take that needs them (or on preroll once the
//! pressure clears). Each shed and re-acquire is recorded
//! (`RenderLoop::loop_cache_events`, `loops_shed_total` /
//! `loops_reacquired_total`, a log line), and `degradationRung` on the §10.1
//! tick reports the rung.
//!
//! Pressure is forced the way prompt04's ladder test forces it: an injected
//! View delay past the 33 ms budget, measured by the loop itself. Items reach
//! the buses through real directives — `show.resync` (the snapshot a control
//! plane sends) and `view.cut` (a take through `on_take`). GPU and video decode
//! are required; a machine without an adapter skips loudly.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{LoopCacheEvent, RenderLoop, Rung, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};

const FRAME_30: Duration = Duration::from_millis(33);
const OVER_BUDGET: Duration = Duration::from_millis(60);
/// A wide budget and a delay just past it: every late frame is ONE missed
/// frame to the watchdog (`ceil(late / budget)`) unless a single render takes
/// longer than 190 ms — so the pattern below never trips the slate on a busy
/// runner. (At the 33 ms budget, 36 ms leaves 30 ms of render headroom, which
/// a loaded machine can spend.)
const WIDE_BUDGET: Duration = Duration::from_millis(200);
const JUST_LATE: Duration = Duration::from_millis(210);

fn media(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/media")
        .join(name)
}

fn directive(
    command: &str,
    sv: u64,
    target: serde_json::Value,
    payload: serde_json::Value,
) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target,
        payload,
    }
}

fn centre_px(bytes: &[u8]) -> [u8; 4] {
    let idx = (((VIEW_H / 2) * VIEW_W + VIEW_W / 2) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

/// Two looping items on two loops: A1 plays `loopA`, A2 plays `loopB`.
fn write_package(dir: &Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    std::fs::copy(media("loop_10.mp4"), dir.join("media/a.mp4")).unwrap();
    std::fs::copy(media("loop_10.mp4"), dir.join("media/b.mp4")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();
    let lp = serde_json::json!({ "periodFrames": 10, "seamless": true });
    std::fs::write(
        dir.join("manifest.json"),
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
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" },
                { "id": "loopA", "kind": "video", "source": "media/a.mp4", "format": "h264", "loop": lp },
                { "id": "loopB", "kind": "video", "source": "media/b.mp4", "format": "h264", "loop": lp }
            ],
            "scenes": [
                { "id": "SA", "elements": [{ "id": "a", "kind": "videoLoop", "z": 1, "assetId": "loopA" }] },
                { "id": "SB", "elements": [{ "id": "b", "kind": "videoLoop", "z": 1, "assetId": "loopB" }] }
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SA" },
                { "id": "A2", "kind": "sceneRef", "sceneRef": "SB" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

/// Load the package, start the show, and put A1 on the View and A2 on the
/// Preview through `show.resync` — the snapshot a control plane sends.
async fn on_air(dir: &Path) -> Option<(Arc<EngineState>, DirectiveHandler, RenderLoop)> {
    write_package(dir);
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.to_string_lossy() }),
        ))
        .await
        .expect("show.load of the ladder package must succeed");
    handler
        .apply(&directive(
            "show.start",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let render = match RenderLoop::new(state.clone()).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("SKIP: no GPU adapter on this machine ({e}); the ladder needs a render loop");
            return None;
        }
    };
    handler
        .apply(&directive(
            nbe_protocol::command::RESYNC,
            3,
            serde_json::json!({}),
            serde_json::json!({
                "showState": "RUNNING",
                "viewItem": "A1",
                "viewItemStartFrame": 0,
                "previewItem": "A2",
                "itemStates": { "A1": "LIVE", "A2": "ARMED" },
                "sceneStates": {},
                "visibleOverlays": [],
                "automationHold": false,
                "stateVersion": 3
            }),
        ))
        .await
        .expect("show.resync must apply");
    assert_eq!(
        render.resident_loops(),
        ["loopA".to_string(), "loopB".to_string()].into(),
        "both loops resident at load"
    );
    Some((state, handler, render))
}

fn sheds(render: &RenderLoop) -> Vec<String> {
    render
        .loop_cache_events
        .iter()
        .filter_map(|e| match e {
            LoopCacheEvent::Shed { asset_id, .. } => Some(asset_id.clone()),
            _ => None,
        })
        .collect()
}

fn reacquires(render: &RenderLoop) -> Vec<(String, &'static str)> {
    render
        .loop_cache_events
        .iter()
        .filter_map(|e| match e {
            LoopCacheEvent::Reacquired {
                asset_id, reason, ..
            } => Some((asset_id.clone(), *reason)),
            _ => None,
        })
        .collect()
}

/// The ladder is a ladder: under sustained pressure rung 1 engages before rung
/// 2, rung 2 is only reached FROM rung 1, and nothing is shed before rung 2.
#[tokio::test]
async fn the_ladder_climbs_in_order_rung_1_before_rung_2() {
    let dir = tempfile::tempdir().unwrap();
    let Some((state, _h, mut render)) = on_air(dir.path()).await else {
        return;
    };
    render.injected_view_delay = Some(OVER_BUDGET);
    let mut rungs = Vec::new();
    let mut shed_at_rung = Vec::new();
    for f in 0..6 {
        let rung_before = state.rung();
        let before = render.loop_cache_events.len();
        render.render_frame(100 + f, Some(FRAME_30));
        if render.loop_cache_events.len() > before {
            shed_at_rung.push(rung_before);
        }
        rungs.push(state.degradation_rung());
    }
    assert_eq!(
        rungs,
        vec![0, 1, 1, 2, 2, 2],
        "rung 1 at 2 late frames, rung 2 at 4 — in that order"
    );
    assert!(
        shed_at_rung.iter().all(|r| *r == Rung::LoopsShed),
        "nothing is shed before rung 2 is in force: {shed_at_rung:?}"
    );
    assert_eq!(
        sheds(&render),
        vec!["loopB".to_string()],
        "rung 2 shed the off-air loop"
    );

    // Pressure gone: 30 on-time frames restore nominal.
    render.injected_view_delay = None;
    for f in 6..(6 + 30) {
        render.render_frame(100 + f, Some(Duration::from_secs(5)));
    }
    assert_eq!(state.rung(), Rung::Nominal);
}

/// Off-air eviction: the previewed loop sheds under rung 2, the shed is
/// recorded, and a take of that item re-acquires it — recorded too.
#[tokio::test]
async fn an_off_air_loop_sheds_under_rung_2_and_is_reacquired_on_take() {
    let dir = tempfile::tempdir().unwrap();
    let Some((state, handler, mut render)) = on_air(dir.path()).await else {
        return;
    };
    render.injected_view_delay = Some(OVER_BUDGET);
    for f in 0..6 {
        render.render_frame(100 + f, Some(FRAME_30));
    }
    assert_eq!(state.rung(), Rung::LoopsShed);
    assert_eq!(
        render.resident_loops(),
        ["loopA".to_string()].into(),
        "the previewed loop is shed"
    );
    assert_eq!(sheds(&render), vec!["loopB".to_string()]);
    assert_eq!(
        state.loops_shed_total.load(Ordering::SeqCst),
        1,
        "the shed is counted"
    );
    assert!(
        reacquires(&render).is_empty(),
        "under rung 2 the preview does not re-preroll it"
    );

    // Take A2 (still under pressure): the View needs loopB, and it comes back.
    handler
        .apply(&directive(
            "view.cut",
            4,
            serde_json::json!({ "itemRef": "A2" }),
            serde_json::json!({ "transition": "cut" }),
        ))
        .await
        .expect("the take must apply");
    render.render_frame(106, Some(FRAME_30));
    assert_eq!(
        reacquires(&render),
        vec![("loopB".to_string(), "take")],
        "re-acquired on take, recorded"
    );
    assert_eq!(state.loops_reacquired_total.load(Ordering::SeqCst), 1);
    assert!(
        render.resident_loops().contains("loopB"),
        "the loop now on air is resident"
    );
    // …and loopA, off air now, is the one rung 2 sheds.
    render.render_frame(107, Some(FRAME_30));
    assert!(
        sheds(&render).contains(&"loopA".to_string()),
        "the old on-air loop, now off air, sheds"
    );
    assert!(render.resident_loops().contains("loopB"));
}

/// The View-continuity guard: with a loop ON AIR and the ladder driven to rung
/// 2 and held there, the on-air loop is never evicted and the View keeps its
/// cadence.
///
/// The pressure is the maximum the View can take without the §10.3 watchdog
/// putting the fallback slate on air: rung 1 by two consecutive misses (the
/// watchdog trips on three), then every other frame late — sustained, never a
/// third consecutive miss. Under consecutive misses the slate goes up first,
/// by design, and covers the loop; `the_on_air_loop_stays_resident_under_consecutive_misses`
/// pins residency there.
#[tokio::test]
async fn the_on_air_loop_is_never_shed_and_the_view_keeps_its_cadence() {
    let dir = tempfile::tempdir().unwrap();
    let Some((state, _h, mut render)) = on_air(dir.path()).await else {
        return;
    };
    let mut px = Vec::new();
    for f in 0..24u64 {
        // late, late, then alternating: rung 1 at frame 1, rung 2 by frame 5.
        let late = f < 2 || f % 2 == 1;
        render.injected_view_delay = late.then_some(JUST_LATE);
        render.render_frame(200 + f, Some(WIDE_BUDGET));
        assert!(
            !state.fallback_active.load(Ordering::SeqCst),
            "frame {f}: the watchdog must not have tripped"
        );
        assert!(
            render.resident_loops().contains("loopA"),
            "frame {f}: the loop feeding the View must stay resident (rung {:?})",
            state.rung()
        );
        px.push(centre_px(&render.readback_view().await));
    }
    assert_eq!(
        state.rung(),
        Rung::LoopsShed,
        "the pressure reached rung 2 and held it"
    );
    assert!(
        sheds(&render).contains(&"loopB".to_string()),
        "rung 2 did shed — the off-air loop"
    );
    assert!(
        !sheds(&render).contains(&"loopA".to_string()),
        "the on-air loop was never shed: {:?}",
        render.loop_cache_events
    );
    assert!(
        reacquires(&render).iter().all(|(a, _)| a != "loopA"),
        "and so never had to be re-acquired: {:?}",
        render.loop_cache_events
    );
    // Cadence: the loop advances frame by frame and repeats every period, as it
    // does unpressured — the View was not frozen or re-timed.
    assert_eq!(px[0], px[10], "the loop repeats every 10 frames");
    assert_eq!(px[3], px[13]);
    assert!(
        px[0..10].windows(2).any(|w| w[0] != w[1]),
        "the loop moves within a period: {px:?}"
    );
}

/// Consecutive misses: the watchdog's slate goes up (§10.3) before rung 2 can,
/// and the on-air loop is still never evicted — whatever covers it, the loop
/// that feeds the View keeps its ring.
#[tokio::test]
async fn the_on_air_loop_stays_resident_under_consecutive_misses() {
    let dir = tempfile::tempdir().unwrap();
    let Some((state, _h, mut render)) = on_air(dir.path()).await else {
        return;
    };
    render.injected_view_delay = Some(OVER_BUDGET);
    for f in 0..24 {
        render.render_frame(300 + f, Some(FRAME_30));
        assert!(
            render.resident_loops().contains("loopA"),
            "frame {f}: the on-air loop stays resident"
        );
    }
    assert_eq!(state.rung(), Rung::LoopsShed);
    assert!(
        state.fallback_active.load(Ordering::SeqCst),
        "consecutive misses engage the slate (§10.3)"
    );
    assert!(!sheds(&render).contains(&"loopA".to_string()));
}
