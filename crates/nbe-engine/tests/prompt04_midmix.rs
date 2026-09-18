//! Prompt 04 Step 1 — mid-mix take continuity.
//!
//! Path: DirectiveHandler::on_take → state.transition → RenderLoop::scene_for.
//!
//! DISCOVERED (Step-0 audit): `on_take` OVERWRITES `state.transition`. A take
//! landing mid-mix sets `from_item` to the old `to_item` (view_item was already
//! flipped) and discards the partial blend: frame N shows blend(A,B), frame N+1
//! shows B full + C ramping 0→1 — a visible pop of magnitude (1-α)·|B−A|.
//!
//! REQUIRED (the honest rule): the new transition starts from the currently
//! displayed blended state — never a jump to from-scene, never black. A cut
//! landing mid-mix keeps its instant semantics (a cut is supposed to snap).

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BLUE: [u8; 4] = [0, 0, 255, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// Continuity bound: first frame of the new take vs the interrupted blend
/// frame. Fixed code renders C@0.0 over the frozen blend, so this is ~0
/// (8-bit rounding only). The old overwrite behavior pops by (1-α)·|B−A|:
/// ~191/127/64 channels at 25/50/75% — see per-test MEASURED notes.
const CONTINUITY_MAX_DIST: u8 = 3;

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

/// Three solid scenes (red/blue/green) + a real PNG fallback slate.
fn make_package3(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let mut png = Vec::new();
    let img = image::RgbaImage::from_pixel(64, 32, image::Rgba([10, 20, 30, 255]));
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": {"width":1920,"height":1080,"frameRate":30,"colorSpace":"rec709"},
                "audio": {"sampleRate":48000,"loudnessTargetLufs":-16,"truePeakDbtp":-1.5},
                "fallbackAssetId": "slate"
            },
            "qualityProfile": "potato",
            "assets": [{ "id": "slate", "kind": "image", "source": "media/slate.png" }],
            "scenes": [
                { "id": "SCN_RED", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ff0000" } }
                ]},
                { "id": "SCN_BLUE", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#0000ff" } }
                ]},
                { "id": "SCN_GREEN", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#00ff00" } }
                ]},
                { "id": "SCN_YELLOW", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#ffff00" } }
                ]}
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_RED" },
                { "id": "A2", "kind": "sceneRef", "sceneRef": "SCN_BLUE" },
                { "id": "A3", "kind": "sceneRef", "sceneRef": "SCN_GREEN" },
                { "id": "A4", "kind": "sceneRef", "sceneRef": "SCN_YELLOW" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

async fn loaded_engine(dir: &std::path::Path) -> (Arc<EngineState>, DirectiveHandler, RenderLoop) {
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
        .unwrap();
    let render = RenderLoop::new(state.clone()).await.unwrap();
    (state, handler, render)
}

fn centre_px(bytes: &[u8]) -> [u8; 4] {
    let idx = (((VIEW_H / 2) * VIEW_W + VIEW_W / 2) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]]
}

/// Max abs channel distance over RGB (alpha ignored).
fn chan_dist(a: [u8; 4], b: [u8; 4]) -> u8 {
    (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0)
}

/// Block until the master clock reaches at least `target`; return the frame
/// observed at return. Never skips: the caller derives the take's start frame
/// (master+1) from the actual return value, so scheduling jitter moves the
/// interrupt point but never breaks the frame pairing.
fn wait_master_at_least(state: &Arc<EngineState>, target: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(m) = state.master_frame() {
            if m >= target {
                return m;
            }
        }
        assert!(
            Instant::now() < deadline,
            "master clock never reached {target}; test environment stalled"
        );
        std::thread::sleep(Duration::from_micros(500));
    }
}

async fn take_mix(handler: &DirectiveHandler, sv: u64, item: &str, duration: u64) {
    handler
        .apply(&directive(
            "view.take",
            sv,
            serde_json::json!({ "itemRef": item }),
            serde_json::json!({ "transition": "mix", "durationFrames": duration }),
        ))
        .await
        .unwrap();
}

fn transition_start(state: &Arc<EngineState>) -> u64 {
    state
        .transition
        .lock()
        .unwrap()
        .as_ref()
        .expect("the take must arm a transition")
        .start_frame
}

/// Interrupt a 20-frame A1→A2 mix near `target_offset` (frames after S1) with
/// a 20-frame take to A3. Returns (S1, S2, interrupted blend pixel, first new
/// pixel, second new pixel, state, tempdir guard).
///
/// Timing: frames are pure functions of state, but the TAKE's start frame
/// comes from the wall clock — so both takes happen back-to-back with no GPU
/// work between them (a readback costs ~40 ms, enough for the master to run
/// away). The interrupted blend frame is rendered AFTERWARDS by restoring the
/// snapshotted first transition: same state + same frame = same pixels (the
/// determinism the Step-1 suite already pins).
async fn interrupt_midmix(
    target_offset: u64,
) -> (
    u64,
    u64,
    [u8; 4],
    [u8; 4],
    [u8; 4],
    Arc<EngineState>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    let s1 = transition_start(&state);
    let first = state
        .transition
        .lock()
        .unwrap()
        .clone()
        .expect("the first take must arm a transition");

    let m = wait_master_at_least(&state, s1 + target_offset - 1);
    take_mix(&handler, 3, "A3", 20).await;
    let s2 = transition_start(&state);
    assert_eq!(s2, m + 1, "AC-17: a take lands on the next frame boundary");
    // Regime, not wall timing: the interrupt must land strictly mid-mix
    // (elapsed ≥1 so the blend exists, and < duration so it is still in
    // flight). The old ±3-frames-of-target assert flaked on loaded machines
    // whose clock runs between the wait and the take; continuity below is
    // computed against the RE-RENDERED interrupted frame, so it holds at any
    // actual offset — the percent in each test's name is the target, not a
    // pin. Test names keep the intent; this assert keeps the regime.
    assert!(
        s2 > s1 && s2 - s1 < 20,
        "the interrupt must land strictly mid-mix: S1={s1} S2={s2}"
    );

    render.render_frame(s2, None);
    let first_new = centre_px(&render.readback_view().await);
    render.render_frame(s2 + 1, None);
    let second_new = centre_px(&render.readback_view().await);

    // Re-render the interrupted frame under the snapshotted first transition.
    *state.view_item.lock().unwrap() = Some("A2".into());
    *state.transition.lock().unwrap() = Some(first);
    render.render_frame(s2 - 1, None);
    let interrupted = centre_px(&render.readback_view().await);
    assert!(
        interrupted[0] > 0 && interrupted[2] > 0 && interrupted[1] == 0,
        "precondition: frame S2-1={} is a genuine red/blue blend, got {interrupted:?}",
        s2 - 1,
    );

    // The TempDir guard is returned so the package path stays valid.
    (s1, s2, interrupted, first_new, second_new, state, dir)
}

#[tokio::test]
async fn midmix_take_at_25pct_is_continuous() {
    // MEASURED on overwrite behavior (RED): interrupted blend [204, 0, 51]
    // (interrupt landed 1 frame early, α≈0.20); the new take's first frame
    // showed [0, 0, 255] (B full) — channel distance 204, the pop
    // (1−α)·|B−A|. Fixed code must render ≈[204, 0, 51].
    let (_s1, _s2, interrupted, first_new, _second, _st, _dir) = interrupt_midmix(5).await;
    let d = chan_dist(interrupted, first_new);
    assert!(
        d <= CONTINUITY_MAX_DIST,
        "25%: first new frame {first_new:?} must continue interrupted blend \
         {interrupted:?} (dist {d} > {CONTINUITY_MAX_DIST})"
    );
}

#[tokio::test]
async fn midmix_take_at_50pct_is_continuous() {
    // MEASURED on overwrite behavior (RED): interrupted blend [140, 0, 115]
    // (α≈0.45); new first frame [0, 0, 255] — distance 140, the pop
    // (1−α)·|B−A|.
    let (_s1, _s2, interrupted, first_new, _second, _st, _dir) = interrupt_midmix(10).await;
    let d = chan_dist(interrupted, first_new);
    assert!(
        d <= CONTINUITY_MAX_DIST,
        "50%: first new frame {first_new:?} must continue interrupted blend \
         {interrupted:?} (dist {d} > {CONTINUITY_MAX_DIST})"
    );
}

#[tokio::test]
async fn midmix_take_at_75pct_is_continuous() {
    // MEASURED on overwrite behavior (RED): interrupted blend [77, 0, 178]
    // (α≈0.70); new first frame [0, 0, 255] — distance 77, the pop
    // (1−α)·|B−A|.
    let (_s1, _s2, interrupted, first_new, _second, _st, _dir) = interrupt_midmix(15).await;
    let d = chan_dist(interrupted, first_new);
    assert!(
        d <= CONTINUITY_MAX_DIST,
        "75%: first new frame {first_new:?} must continue interrupted blend \
         {interrupted:?} (dist {d} > {CONTINUITY_MAX_DIST})"
    );
}

#[tokio::test]
async fn midmix_take_honors_ac17_new_content_by_start_plus_1() {
    // AC-17: visible change by acceptance+2. The take lands at start =
    // acceptance+1; the new item (green — absent from the red/blue blend)
    // must already be present at start+1.
    let (_s1, _s2, interrupted, first_new, second_new, _state, _dir) = interrupt_midmix(10).await;
    // `s2` is returned (not re-read): the helper restores the first
    // transition to re-render the interrupted frame, so state no longer
    // holds the second take.
    assert!(
        second_new[1] >= 8,
        "new content (green) must be present at start+1: got {second_new:?}"
    );
    assert_ne!(
        second_new, interrupted,
        "the view must have visibly changed by start+1"
    );
    let d = chan_dist(interrupted, first_new);
    assert!(
        d <= CONTINUITY_MAX_DIST,
        "continuity still holds on the AC-17 path: {first_new:?} vs {interrupted:?}"
    );
}

#[tokio::test]
async fn cut_midmix_snaps_instantly() {
    // GAP-8 golden: a cut landing mid-mix keeps instant semantics — the new
    // item is full-on at its start frame. Pinned, not changed.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    let s1 = transition_start(&state);
    render.render_frame(s1 + 10, None);
    let blend = centre_px(&render.readback_view().await);
    assert!(
        blend[0] > 0 && blend[2] > 0,
        "precondition: mid-mix frame is a blend, got {blend:?}"
    );

    let m = wait_master_at_least(&state, s1 + 9);
    handler
        .apply(&directive(
            "view.cut",
            3,
            serde_json::json!({ "itemRef": "A3" }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let s2 = transition_start(&state);
    assert_eq!(s2, m + 1, "AC-17 holds for the cut too");
    render.render_frame(s2, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        GREEN,
        "a cut is supposed to snap: full new item at its start frame"
    );
}

#[tokio::test]
async fn completed_mix_then_take_renders_correctly() {
    // GAP-7 sequential golden: a take AFTER a mix completed is an ordinary
    // take — no frozen underlay may leak into it.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    let s1 = transition_start(&state);
    render.render_frame(s1 + 20, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        BLUE,
        "precondition: the first mix ran to completion"
    );

    let _ = wait_master_at_least(&state, s1 + 20);
    take_mix(&handler, 3, "A3", 10).await;
    let s2 = transition_start(&state);
    assert!(
        s2 > s1 + 20,
        "the second take starts after the first mix completed: S1={s1} S2={s2}"
    );
    render.render_frame(s2 + 5, None);
    let mid = centre_px(&render.readback_view().await);
    assert!(
        mid[2] > 0 && mid[1] > 0,
        "second mix blends blue→green with no red underlay leaking through, got {mid:?}"
    );
    assert_eq!(mid[0], 0, "no red may survive a completed mix: {mid:?}");
    render.render_frame(s2 + 10, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        GREEN,
        "second mix completes into the incoming scene"
    );
}

/// The collapse fix keeps the underlay FLAT (no nesting): chained mid-mix
/// interrupts extend one layer list. Depth is therefore 1 by construction —
/// this pins the exact flattened contents instead of walking a chain.
fn assert_collapsed_underlay(t: &nbe_engine::scene::Transition, expected: &[(&str, f32, u64)]) {
    let Some(u) = t.underlay.as_ref() else {
        panic!("a mid-mix take must leave an underlay");
    };
    assert_eq!(
        u.layers.len(),
        expected.len(),
        "collapsed underlay must hold exactly one frozen layer per chained \
         interrupt plus the opaque base (no nesting to hide behind): got {:?}",
        u.layers
            .iter()
            .map(|l| (l.item.clone(), l.alpha, l.t0))
            .collect::<Vec<_>>()
    );
    for (i, (layer, (item, alpha, t0))) in u.layers.iter().zip(expected).enumerate() {
        assert_eq!(layer.item, *item, "frozen layer {i} item");
        assert!(
            (layer.alpha - alpha).abs() < 1e-6,
            "frozen layer {i} alpha: got {}, want {alpha}",
            layer.alpha
        );
        assert_eq!(layer.t0, *t0, "frozen layer {i} t0");
    }
}

#[tokio::test]
async fn three_deep_takes_stay_continuous_and_bounded() {
    // A→B@S1, →C@S2 (mid-mix), →D@S3 (mid-mix again). Unfixed code nests the
    // underlays (depth 2 here, growing per interrupt: O(n²) clone on the take
    // path, O(depth) re-walk every frame). The fix collapses to depth ≤ 1
    // while keeping continuity at EVERY interrupt.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    let s1 = transition_start(&state);
    let t1 = state.transition.lock().unwrap().clone().unwrap();

    let m2 = wait_master_at_least(&state, s1 + 4);
    take_mix(&handler, 3, "A3", 20).await;
    let s2 = transition_start(&state);
    assert_eq!(s2, m2 + 1, "AC-17: second take lands on the next boundary");
    assert!(
        s2 > s1 && s2 - s1 < 20,
        "second interrupt must land strictly mid-mix: S1={s1} S2={s2}"
    );
    let t2 = state.transition.lock().unwrap().clone().unwrap();

    let m3 = wait_master_at_least(&state, s2 + 4);
    take_mix(&handler, 4, "A4", 20).await;
    let s3 = transition_start(&state);
    assert_eq!(s3, m3 + 1, "AC-17: third take lands on the next boundary");
    assert!(
        s3 > s2 && s3 - s2 <= 18,
        "third interrupt must land strictly mid-mix: S2={s2} S3={s3}"
    );
    assert!(
        !t2.is_complete(s3 - 1),
        "precondition: the third take interrupts a live mix"
    );
    let t3 = state.transition.lock().unwrap().clone().unwrap();

    // Continuity at the THIRD interrupt: T3[S3] vs T2[S3-1].
    render.render_frame(s3, None);
    let first3 = centre_px(&render.readback_view().await);
    *state.view_item.lock().unwrap() = Some("A3".into());
    *state.transition.lock().unwrap() = Some(t2.clone());
    render.render_frame(s3 - 1, None);
    let blend2 = centre_px(&render.readback_view().await);
    assert!(
        blend2[1] > 0 && (blend2[0] > 0 || blend2[2] > 0),
        "precondition: frame S3-1={} is a genuine 3-layer composite (must \
         carry green from C plus red/blue beneath), got {blend2:?}",
        s3 - 1,
    );
    let d3 = chan_dist(blend2, first3);
    assert!(
        d3 <= CONTINUITY_MAX_DIST,
        "3rd interrupt: first new frame {first3:?} must continue interrupted \
         composite {blend2:?} (dist {d3} > {CONTINUITY_MAX_DIST})"
    );

    // Continuity at the SECOND interrupt: T2[S2] vs T1[S2-1].
    *state.view_item.lock().unwrap() = Some("A2".into());
    *state.transition.lock().unwrap() = Some(t1.clone());
    render.render_frame(s2 - 1, None);
    let blend1 = centre_px(&render.readback_view().await);
    *state.transition.lock().unwrap() = Some(t2.clone());
    render.render_frame(s2, None);
    let first2 = centre_px(&render.readback_view().await);
    let d2 = chan_dist(blend1, first2);
    assert!(
        d2 <= CONTINUITY_MAX_DIST,
        "2nd interrupt: first new frame {first2:?} must continue interrupted \
         blend {blend1:?} (dist {d2} > {CONTINUITY_MAX_DIST})"
    );

    // Bounded depth: the collapse. One flat layer list — opaque base plus
    // one frozen layer per chained interrupt — with exact items, frozen
    // alphas, and t0s. (Unfixed code nested `Underlay` in `Underlay` here:
    // depth 2, RED-seen.)
    assert_collapsed_underlay(
        &t3,
        &[
            ("A1", 1.0, 0),
            ("A2", t1.progress(s2 - 1), s1),
            ("A3", t2.progress(s3 - 1), s2),
        ],
    );
}

#[tokio::test]
async fn midmix_take_queues_fresh_take_at_new_t0() {
    // Audio mid-mix honesty pin: a take REPLACES the clip-bus source (it
    // restarts through silence via `swap_source_through_silence`) rather than
    // crossfading out of the current video blend. The observable contract on
    // this path is the queue: a second take pushes a FRESH TakeItem carrying
    // the new item and the new t0 — never a mutation of the in-flight one.
    // (Harness mirrors prompt06's `audio_commands` queue reads.)
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, _render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    let s1 = transition_start(&state);
    let m2 = wait_master_at_least(&state, s1 + 9);
    take_mix(&handler, 3, "A3", 20).await;
    let s2 = transition_start(&state);
    assert_eq!(s2, m2 + 1, "AC-17 holds for the audio path too");

    let cmds = state.audio_commands.lock().unwrap();
    assert_eq!(
        cmds.len(),
        2,
        "each take publishes exactly one audio intent, so a mid-mix take \
         must leave two queued TakeItems, got {}",
        cmds.len()
    );
    let item_of = |c: &nbe_engine::audio_control::AudioCommand| match c {
        nbe_engine::audio_control::AudioCommand::TakeItem {
            item_ref,
            asset_id: _,
            t0,
            mode: _,
            ramp_ms: _,
            crossfade_frames: _,
        } => (item_ref.clone(), *t0),
        other => panic!("take path must queue TakeItem, got {other:?}"),
    };
    let (first_item, first_t0) = item_of(&cmds[0]);
    let (second_item, second_t0) = item_of(&cmds[1]);
    assert_eq!((first_item.as_str(), first_t0), ("A2", s1));
    assert_eq!(
        (second_item.as_str(), second_t0),
        ("A3", s2),
        "the mid-mix take must push a fresh TakeItem for the NEW item at the \
         NEW t0 (replace, not crossfade-from-blend)"
    );
}

/// Fixture path shared with prompt05: the engine tests reuse the cadence
/// clips, no binaries added here.
fn media(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/media")
        .join(name)
}

/// A1 = `cadence_12.mp4` as a full-frame clip (frame N is red = N*20 —
/// prompt05's pin), A2 = blue solid, A3 = green solid, plus the PNG slate.
fn make_package_video(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    std::fs::copy(media("cadence_12.mp4"), dir.join("media/v.mp4"))
        .expect("cadence_12.mp4 must be reachable from engine tests");
    let mut png = Vec::new();
    let img = image::RgbaImage::from_pixel(64, 32, image::Rgba([10, 20, 30, 255]));
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": {"width":1920,"height":1080,"frameRate":30,"colorSpace":"rec709"},
                "audio": {"sampleRate":48000,"loudnessTargetLufs":-16,"truePeakDbtp":-1.5},
                "fallbackAssetId": "slate"
            },
            "qualityProfile": "potato",
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png" },
                { "id": "V", "kind": "video", "source": "media/v.mp4", "format": "h264" }
            ],
            "scenes": [
                { "id": "SCN_VID", "elements": [
                    { "id": "main", "kind": "clip", "z": 1, "assetId": "V" }
                ]},
                { "id": "SCN_BLUE", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#0000ff" } }
                ]},
                { "id": "SCN_GREEN", "elements": [
                    { "id": "fill", "kind": "graphic", "z": 1, "templateId": "t",
                      "fields": { "color": "#00ff00" } }
                ]}
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_VID" },
                { "id": "A2", "kind": "sceneRef", "sceneRef": "SCN_BLUE" },
                { "id": "A3", "kind": "sceneRef", "sceneRef": "SCN_GREEN" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn underlay_video_advances_on_its_own_clock_while_alpha_stays_frozen() {
    // Live-clock pin for the frozen blend: A1 is a REAL video element, so the
    // underlay's from-layer must advance with its own t0 across frames while
    // its frozen α does not move. Blue and green carry no red, so the centre
    // red channel is the video's alone: it must vary frame-over-frame (the
    // clip advances) while `scene_for` reports the frozen alphas constant and
    // every layer's t0 pinned to its own item's start.
    let dir = tempfile::tempdir().unwrap();
    make_package_video(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    // A1 goes on air without a take, so its t0 is the initial 0 — the same
    // convention the solid tests use, and what the t0 assertions below pin.
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    let s1 = transition_start(&state);
    let t1 = state.transition.lock().unwrap().clone().unwrap();
    let m2 = wait_master_at_least(&state, s1 + 4);
    take_mix(&handler, 3, "A3", 20).await;
    let s2 = transition_start(&state);
    assert_eq!(s2, m2 + 1, "AC-17 holds on the video path too");
    assert!(
        s2 > s1 && s2 - s1 < 20,
        "the interrupt must land strictly mid-mix: S1={s1} S2={s2}"
    );
    let frozen = t1.progress(s2 - 1);
    assert!(
        (0.05..0.95).contains(&frozen),
        "precondition: the interrupt must freeze a genuine partial blend, \
         got α={frozen}"
    );

    // scene_for level: frozen alphas constant, fresh alpha moving, t0s pinned.
    let layers_at = |frame: u64| {
        render
            .scene_for(nbe_engine::render::Bus::View, frame)
            .into_iter()
            .map(|b| (b.alpha, b.t0))
            .collect::<Vec<_>>()
    };
    let early = layers_at(s2);
    let late = layers_at(s2 + 3);
    assert_eq!(
        early.len(),
        3,
        "frozen blend + fresh to_item, got {early:?}"
    );
    assert_eq!(late.len(), 3, "layer count must not move, got {late:?}");
    for (i, (e, l)) in early.iter().zip(late.iter()).enumerate() {
        assert_eq!(
            e.1, l.1,
            "layer {i} t0 must stay pinned to its own item's start: {e:?} vs {l:?}"
        );
    }
    assert_eq!((early[0].1, early[1].1, early[2].1), (0, s1, s2));
    assert!(
        (early[0].0 - 1.0).abs() < 1e-6 && (early[1].0 - frozen).abs() < 1e-6,
        "the underlay layers must carry the frozen alphas (1.0, {frozen}), \
         got {early:?}"
    );
    assert!(
        (late[0].0 - early[0].0).abs() < 1e-6 && (late[1].0 - early[1].0).abs() < 1e-6,
        "frozen alphas must not move across frames: {early:?} vs {late:?}"
    );
    assert!(
        (late[2].0 - early[2].0).abs() > 1e-6,
        "the fresh to_item alpha must keep ramping while the underlay stays \
         frozen: {early:?} vs {late:?}"
    );

    // Pixel level: red is the video's alone — it must vary (clip advances
    // under the frozen blend) while the blend weights do not.
    let mut reds = Vec::new();
    for f in s2..=s2 + 8 {
        render.render_frame(f, None);
        reds.push(centre_px(&render.readback_view().await)[0]);
    }
    assert!(
        reds[0] > 10,
        "precondition: the video must actually be on air under the blend \
         (red present at S2), got {reds:?}"
    );
    let spread = reds.iter().max().unwrap() - reds.iter().min().unwrap();
    assert!(
        spread >= 5,
        "the frozen-α blend's content must change frame-over-frame as the \
         underlying clip advances on its own clock (red spread {spread} \
         across {reds:?})"
    );
}

// ---------------------------------------------------------------------------
// Step 3 — GAP-6 duration edges + GAP-11 start+1 bisection (stopped clock,
// start_frame == 0, fully deterministic — no wall-clock).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn default_15_mix_path_blends_midway_and_completes_at_s_plus_15() {
    // GAP-6 golden: a mix take with NO durationFrames takes the §7.9
    // default-15 path — duration==15, blend mid-way, complete at s+15.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    handler
        .apply(&directive(
            "view.take",
            2,
            serde_json::json!({ "itemRef": "A2" }),
            serde_json::json!({ "transition": "mix" }),
        ))
        .await
        .unwrap();
    let t = state.transition.lock().unwrap().clone().unwrap();
    assert_eq!(
        t.duration_frames, 15,
        "a mix with no durationFrames must take the §7.9 default-15 path"
    );
    let s = t.start_frame;

    render.render_frame(s + 7, None);
    let mid = centre_px(&render.readback_view().await);
    assert!(
        mid[0] > 0 && mid[2] > 0,
        "mid-default-mix must blend both scenes; got {mid:?}"
    );
    render.render_frame(s + 15, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        BLUE,
        "default-15 mix must complete at s+15"
    );
}

#[tokio::test]
async fn zero_duration_mix_renders_as_cut() {
    // GAP-6 golden: duration-0 mix collapses to a cut — render(s) is the new
    // item full. FALSIFICATION NOTE: the `duration_frames == 0` early-return
    // in progress() is unreachable via the general path (elapsed>=0 always
    // wins for frame>=start), so this is falsified by forcing 0.0 for
    // duration==0 (progress returns 0.0, is_complete false, the mix branch
    // renders old@1.0 + new@0.0 = old full) and watching this test fail
    // with old-full instead of new-full — NOT by a mutation that changes
    // nothing.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 0).await;
    let t = state.transition.lock().unwrap().clone().unwrap();
    assert_eq!(t.duration_frames, 0);
    assert!(
        t.is_complete(t.start_frame),
        "a zero-duration mix must be complete at its start frame"
    );
    render.render_frame(t.start_frame, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        BLUE,
        "zero-duration mix renders as cut: full new item at s"
    );
}

#[tokio::test]
async fn one_frame_mix_completes_at_s_plus_1() {
    // GAP-6 golden: a 1-frame mix is complete at s+1.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 1).await;
    let t = state.transition.lock().unwrap().clone().unwrap();
    assert_eq!(t.duration_frames, 1);
    render.render_frame(t.start_frame + 1, None);
    assert_eq!(
        centre_px(&render.readback_view().await),
        BLUE,
        "one-frame mix must complete at s+1"
    );
}

#[tokio::test]
async fn over_max_mix_accepted_without_clamp() {
    // GAP-6 golden + NO-CLAMP DECISION RECORD: there is deliberately NO clamp
    // anywhere on duration_frames (scene.rs Transition carries the raw u64;
    // directive.rs passes it through), so a 10000-frame mix is ACCEPTED and
    // progress stays sane at its midpoint (0.5). If a max clamp ever lands
    // (schema max 600/120), this test must be updated to pin the clamped
    // value instead — the decision today is accept, recorded here.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 10_000).await;
    let t = state.transition.lock().unwrap().clone().unwrap();
    assert_eq!(
        t.duration_frames, 10_000,
        "over-max mix accepted without clamp (no-clamp decision)"
    );
    let p = t.progress(t.start_frame + 5_000);
    assert!(
        (p - 0.5).abs() < 1e-6,
        "progress sane at midpoint of a 10000-frame mix: got {p}"
    );
    render.render_frame(t.start_frame + 5_000, None);
    let mid = centre_px(&render.readback_view().await);
    assert!(
        mid[0] > 0 && mid[2] > 0,
        "mid-over-max-mix must blend both scenes; got {mid:?}"
    );
}

#[tokio::test]
async fn mix_first_mixed_frame_bisected_at_start_plus_1() {
    // GAP-11 golden: the mix's first mixed frame (start+1) is a PARTIAL
    // blend — neither old-full nor new-full. The existing
    // `mix_interpolates_across_its_duration_and_never_mid_frame` jumps to
    // start+5; this bisects start+1 (progress 0.1 on a 10-frame mix).
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, mut render) = loaded_engine(dir.path()).await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 10).await;
    let t = state.transition.lock().unwrap().clone().unwrap();
    let s = t.start_frame;
    assert!(
        (t.progress(s + 1) - 0.1).abs() < 1e-6,
        "progress at start+1 of a 10-frame mix must be 0.1"
    );
    render.render_frame(s + 1, None);
    let px = centre_px(&render.readback_view().await);
    assert!(
        px[0] > 0 && px[2] > 0,
        "start+1 must blend both scenes; got {px:?}"
    );
    assert_ne!(
        px,
        [255, 0, 0, 255],
        "start+1 must not be old-full: got {px:?}"
    );
    assert_ne!(px, BLUE, "start+1 must not be new-full yet: got {px:?}");
}

#[tokio::test]
async fn interrupt_at_progress_zero_carries_no_dead_layer() {
    // Back-to-back takes: the old blend rendered zero frames or (on a loaded
    // machine that ticked between the takes) a few. Either measured regime
    // has an exact shape — progress-0 skips the dead zero-alpha layer,
    // mid-blend keeps base plus frozen to_item — so the test branches on
    // S1/S2 instead of assuming adjacency and flaking on load.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, _render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    let s1 = transition_start(&state);
    // Second take immediately after: the wall clock decides whether the old
    // blend rendered zero frames or more. Branch on the MEASURED regime
    // instead of assuming adjacency — a loaded machine may tick between the
    // two takes, and either regime has an exact expected shape.
    take_mix(&handler, 3, "A3", 20).await;
    let s2 = transition_start(&state);
    let guard = state.transition.lock().unwrap();
    let t = guard.as_ref().expect("second take arms a transition");
    assert_eq!(t.to_item, "A3", "new transition targets the second take");
    if s2 <= s1 + 1 {
        // Progress-0 regime: the frozen to_item would be a zero-alpha dead
        // layer carried for the whole new mix, so construction skips it. The
        // opaque base remains: displayed frame IS old-from at full.
        let underlay = t.underlay.as_ref().expect("base layer retained");
        assert_eq!(
            underlay.layers.len(),
            1,
            "only the opaque base survives a progress-0 interrupt, got {:?}",
            underlay
                .layers
                .iter()
                .map(|l| (l.item.clone(), l.alpha))
                .collect::<Vec<_>>()
        );
        assert_eq!(underlay.layers[0].item, "A1", "base is the displayed scene");
        assert_eq!(underlay.layers[0].alpha, 1.0, "base is opaque");
    } else {
        // Mid-blend regime (clock ticked between takes): base plus the
        // frozen to_item at positive alpha — the shape the 25/50/75% tests
        // pin by pixels; here pinned structurally.
        let underlay = t.underlay.as_ref().expect("underlay retained");
        assert_eq!(underlay.layers.len(), 2, "base plus frozen to_item");
        assert_eq!(underlay.layers[0].item, "A1");
        assert!(
            underlay.layers[1].alpha > 0.0,
            "frozen to_item carries positive alpha, got {:?}",
            underlay.layers[1].alpha
        );
    }
    drop(guard);
}

#[tokio::test]
async fn interrupt_before_first_blend_carries_no_underlay() {
    // Degenerate back-to-back: the old transition rendered nothing yet (no
    // from_item known — first take of the show — and frozen progress 0), so
    // the layer list is empty and construction yields None, not an empty
    // underlay struct. The new mix starts clean, exactly as a fresh take.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, _render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    // No view_item set: the first take has no from_item.
    take_mix(&handler, 2, "A2", 20).await;
    let s1 = transition_start(&state);
    take_mix(&handler, 3, "A3", 20).await;
    let s2 = transition_start(&state);
    let guard = state.transition.lock().unwrap();
    let t = guard.as_ref().expect("second take arms a transition");
    if s2 <= s1 + 1 {
        // Progress-0 regime: old rendered nothing (no from_item, frozen
        // progress 0), so the layer list is empty and construction yields
        // None — not an empty underlay struct. The new mix starts clean.
        assert!(
            t.underlay.is_none(),
            "nothing rendered yet means no underlay, got {:?}",
            t.underlay.as_ref().map(|u| u.layers.len())
        );
    } else {
        // Mid-blend regime: no from_item, but the frozen to_item carries
        // positive alpha — a single-layer underlay, base absent.
        let underlay = t.underlay.as_ref().expect("frozen to_item retained");
        assert_eq!(underlay.layers.len(), 1, "only the frozen to_item");
        assert_eq!(underlay.layers[0].item, "A2");
        assert!(underlay.layers[0].alpha > 0.0, "frozen alpha positive");
    }
}

#[tokio::test]
async fn stop_clears_inflight_transition() {
    // A mix interrupted by show.stop must not resume stale on the next
    // start: the clock restarts from zero, so in-flight t0s would render
    // against the wrong timeline.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, _render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    assert!(
        state.transition.lock().unwrap().is_some(),
        "precondition: mix in flight"
    );
    handler
        .apply(&directive(
            "show.stop",
            3,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert!(
        state.transition.lock().unwrap().is_none(),
        "stop clears the in-flight transition"
    );
}

#[tokio::test]
async fn resync_without_view_item_key_clears_inflight_transition() {
    // Key ABSENT (not null): an empty bus holds no transition, so the clear
    // must not hide inside the viewItem-present branch.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, _render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    assert!(
        state.transition.lock().unwrap().is_some(),
        "precondition: mix in flight"
    );
    handler
        .apply(&directive(
            nbe_protocol::command::RESYNC,
            3,
            serde_json::json!({}),
            serde_json::json!({ "showState": "RUNNING" }),
        ))
        .await
        .unwrap();
    assert!(
        state.transition.lock().unwrap().is_none(),
        "key-absent resync clears the in-flight transition"
    );
}

#[tokio::test]
async fn chained_interrupts_stay_within_the_layer_cap() {
    // Adversarial take rate: 20 back-to-back 600-frame mixes (nothing can
    // complete between takes). Each interrupt appends exactly one frozen
    // layer; past MAX_UNDERLAY_LAYERS the oldest non-base layer drops, so
    // the composite stays bounded no matter the take rate. The base (index
    // 0) is never the one dropped — continuity's anchor survives.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, _render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    for sv in 2..22u64 {
        let item = if sv % 2 == 0 { "A2" } else { "A3" };
        take_mix(&handler, sv, item, 600).await;
        // Let the master advance so each interrupt lands mid-blend (progress
        // > 0): back-to-back takes on a frozen clock all interrupt at
        // progress 0 and never grow the chain, which would leave the cap
        // unexercised. ~100 ms ≈ 3 frames at 30 fps.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let guard = state.transition.lock().unwrap();
    let t = guard.as_ref().expect("last take arms a transition");
    let underlay = t
        .underlay
        .as_ref()
        .expect("chained interrupts carry an underlay");
    // Exact 8, not merely ≤ 8: 19 chained interrupts must have HIT the cap
    // (20 takes ≈ 60 frames of progress spread, far past 8 layers), so a
    // vacuous pass — chain never growing — fails here instead of hiding.
    assert_eq!(
        underlay.layers.len(),
        8,
        "cap must fire under adversarial take rate"
    );
    assert_eq!(
        underlay.layers[0].item, "A1",
        "base anchor survives the cap"
    );
    assert_eq!(underlay.layers[0].alpha, 1.0, "base stays opaque");
}

#[tokio::test]
async fn failed_quiesce_still_clears_inflight_transition() {
    // The Err path out of show.stop's quiescence arm must clear the
    // transition exactly like the Ok path: a failed finalize withholds the
    // ack but must not strand stale t0s for the next start.
    //
    // WHICH loud failure arrives is machine-dependent, and this comment used to
    // say the opposite — "E_RECORD_INPUT deterministically on every machine (no
    // encoder ...)". CI refuted it (run 35339778904): `RecordSession::open` is
    // lazy, so on a runner with no hardware H.264 encoder the finalize opens the
    // encoder first and fails `E_NO_HARDWARE_ENCODER` before it can reach the
    // no-video check. Both are loud finalize failures and either satisfies this
    // test's actual claim, which is about the Err path clearing the transition —
    // not about which Err it was. Accepting both keeps the test running
    // EVERYWHERE rather than capability-skipping it off the encoder-less runner,
    // which would have cost the coverage this assertion exists for.
    let dir = tempfile::tempdir().unwrap();
    make_package3(dir.path());
    let (state, handler, _render) = loaded_engine(dir.path()).await;
    state.clock.lock().unwrap().start();
    *state.view_item.lock().unwrap() = Some("A1".into());

    take_mix(&handler, 2, "A2", 20).await;
    assert!(
        state.transition.lock().unwrap().is_some(),
        "precondition: mix in flight"
    );
    let recdir = tempfile::tempdir().unwrap();
    let tap = std::sync::Arc::new(nbe_engine::record::AudioTap::new());
    let session = nbe_engine::record::RecordSession::open(
        recdir.path(),
        "s",
        "ep01",
        "20260918T000000Z",
        640,
        360,
        30,
        tap,
        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("session open must succeed");
    *state.record_session.lock().unwrap() = Some(session);
    *state.record_state.lock().unwrap() = nbe_engine::state::RecordState::Recording;

    let err = handler
        .apply(&directive(
            "show.stop",
            3,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .expect_err("empty-take finalize must fail, ack withheld");
    let token = err.to_string();
    assert!(
        token.contains("E_RECORD_INPUT") || token.contains("E_NO_HARDWARE_ENCODER"),
        "expected a loud finalize failure (E_RECORD_INPUT where an encoder exists, \
         E_NO_HARDWARE_ENCODER where one does not), got: {err}"
    );
    assert!(
        state.transition.lock().unwrap().is_none(),
        "failed quiescence clears the in-flight transition exactly like the Ok path"
    );
}
