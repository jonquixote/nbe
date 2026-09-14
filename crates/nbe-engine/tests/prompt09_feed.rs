//! Prompt 09 WU-tap (SPEC §9.3): main-loop record feed.
//!
//! TDD: written BEFORE `record::feed` exists (RED first).
//!
//! The feed runs AFTER `render_frame` + the unchanged deadline check: with an
//! active session it feeds one frame — readback → `encode_rgba` → `session.push_video`,
//! tap drain → `session.push_audio` — timed on the engine-state `record_tap_ms`
//! counter, never in the render budget. Over budget: SKIP the record frame
//! (record degrades, View never) BEFORE the readback, count
//! `skipped_record_frames` in engine state.
//!
//! Unification: the feed owns the ONE live encoder (opened at VIEW_W/H on the
//! first fed frame) and captures the real SPS/PPS into the metadata-only
//! session — no throwaway black-frame encode, no placeholder sets.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::encode::EncodeSession;
use nbe_engine::record::{feed_record_frame, AudioTap};
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};

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

fn make_package(dir: &std::path::Path) {
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
                ]}
            ],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_RED" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

async fn live_renderer() -> (Arc<EngineState>, RenderLoop) {
    let dir = tempfile::tempdir().unwrap();
    make_package(dir.path());
    // Leak the tempdir: the package load reads assets synchronously during
    // apply, and the renderer holds only GPU textures afterwards.
    let dir = Box::leak(Box::new(dir));
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    let render = RenderLoop::new(state.clone())
        .await
        .expect("wgpu adapter unavailable; tests must FAIL loudly, not skip");
    (state, render)
}

/// Metadata-only session at View geometry (unification premise: the session
/// starts with EMPTY parameter sets — `open_synthetic`'s placeholder sets
/// would mask the feed's real capture — and the feed's live encoder fills
/// them from its first keyframe). `finish` is never called here (AAC/file are
/// other work units' paths).
fn metadata_only_session(dir: &std::path::Path) -> nbe_engine::record::RecordSession {
    nbe_engine::record::RecordSession::open(dir, "show", "ep", "feed-test", VIEW_W, VIEW_H, 30)
        .unwrap()
}

/// Fill the tap through the REAL audio path: master-bus render with the tap
/// attached via `set_record_tap` (the record-start wiring call-site).
fn fill_tap() -> Arc<AudioTap> {
    use nbe_engine::audio::{AudioGraph, BusId, Source};
    let tap = Arc::new(AudioTap::with_capacity(48_000 * 2 * 5));
    let mut g = AudioGraph::new(30);
    g.set_record_tap(tap.clone());
    g.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 440.0,
            amplitude: 0.5,
        }],
    );
    let mut out = vec![0.0f32; 1600 * 2];
    g.render(&mut out, 0);
    assert!(
        !tap.drain().is_empty(),
        "tap must fill before feed drains it"
    );
    g.render(&mut out, 1);
    assert!(!tap.is_empty(), "tap must hold audio for the feed to drain");
    tap
}

fn hw_or_skip() -> bool {
    if !nbe_engine::encode::is_available() {
        eprintln!("SKIP: no hardware H.264 encoder on this machine");
        return false;
    }
    true
}

#[tokio::test]
async fn feed_encodes_view_and_drains_tap_with_monotonic_pts() {
    if !hw_or_skip() {
        return;
    }
    let (_state, mut render) = live_renderer().await;
    let dir = tempfile::tempdir().unwrap();
    let mut session = metadata_only_session(dir.path());
    assert!(
        session.parameter_sets().is_none(),
        "unification: the session starts with EMPTY sets"
    );
    // The feed owns the ONE live encoder: None in, opened at VIEW_W/H on the
    // first fed frame.
    let mut encoder: Option<EncodeSession> = None;
    let tap = fill_tap();
    // Prime the stream: the opening IDR typically arrives while later frames
    // are still being fed, so feed several frames before asserting.
    let budget = Duration::from_secs_f64(1.0 / 30.0);
    let mut total_units = 0usize;
    let mut total_audio = 0usize;
    let mut pts: Vec<f64> = Vec::new();
    for frame in 0..30u64 {
        let t0 = std::time::Instant::now();
        let report = render.render_frame(frame, Some(budget));
        let render_elapsed = t0.elapsed();
        assert!(
            report.view_late_by.is_none(),
            "test scene must render inside budget (frame {frame})"
        );
        let rgba = render.readback_view().await;
        assert_eq!(
            rgba.len(),
            (VIEW_W * VIEW_H * 4) as usize,
            "readback must be full View RGBA"
        );
        let outcome = feed_record_frame(
            &rgba,
            &mut encoder,
            &mut session,
            &tap,
            render_elapsed,
            Some(budget),
            Duration::ZERO,
        );
        assert!(!outcome.skipped, "inside budget the feed must not skip");
        total_units += outcome.units.len();
        total_audio += outcome.audio_samples;
        pts.extend(outcome.units.iter().map(|u| u.pts_seconds));
    }
    assert!(
        total_units > 0,
        "N rendered frames must yield encoded units in the session"
    );
    assert!(
        encoder.is_some(),
        "the feed must own the live encoder after fed frames"
    );
    assert!(
        session.parameter_sets().is_some(),
        "unification: the feed captures the real SPS/PPS from its first keyframe"
    );
    assert!(
        total_audio > 0,
        "tap audio must be drained into the session"
    );
    assert!(tap.is_empty(), "feed must drain the tap each frame");
    let mut prev = f64::NEG_INFINITY;
    for p in &pts {
        assert!(
            *p > prev,
            "PTS must increase monotonically, got {p} after {prev}"
        );
        prev = *p;
    }
}

#[tokio::test]
async fn over_budget_feed_skips_and_view_is_unaffected() {
    if !hw_or_skip() {
        return;
    }
    let (state, mut render) = live_renderer().await;
    let dir = tempfile::tempdir().unwrap();
    let mut session = metadata_only_session(dir.path());
    // Unification: no pre-opened encoder — the feed opens lazily, so a skip
    // must leave it unopened (no hardware touched on the skip path).
    let mut encoder: Option<EncodeSession> = None;
    let tap = fill_tap();
    let dropped_before = state
        .dropped_frames_total
        .load(std::sync::atomic::Ordering::SeqCst);

    // Test seam: a 1 ns budget is already exceeded by any real render, so the
    // feed must SKIP without touching the session or the View accounting.
    let frame = 1u64;
    let report = render.render_frame(frame, Some(Duration::from_secs_f64(1.0 / 30.0)));
    assert!(
        report.view_late_by.is_none(),
        "View itself renders inside its real budget"
    );
    let rgba = render.readback_view().await;
    // Inflated render clock: pretend the View already spent 1 s of a 33 ms
    // budget — the feed must yield, not pile on.
    let outcome = feed_record_frame(
        &rgba,
        &mut encoder,
        &mut session,
        &tap,
        Duration::from_secs(1),
        Some(Duration::from_nanos(1)),
        Duration::ZERO,
    );
    assert!(
        outcome.skipped,
        "over-budget feed must skip the record frame"
    );
    assert!(
        encoder.is_none(),
        "a skipped feed must not open the encoder (no hardware touched)"
    );
    assert!(
        outcome.units.is_empty(),
        "a skipped feed must push no video"
    );
    assert_eq!(
        outcome.audio_samples, 0,
        "a skipped feed must push no audio"
    );
    assert!(
        !tap.is_empty(),
        "a skipped feed must leave the tap for the next frame"
    );
    let dropped_after = state
        .dropped_frames_total
        .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        dropped_before, dropped_after,
        "skipping the record frame must not move View deadline accounting"
    );
}

#[tokio::test]
async fn feed_time_is_tracked_separately_from_render_budget() {
    if !hw_or_skip() {
        return;
    }
    let (state, mut render) = live_renderer().await;
    let dir = tempfile::tempdir().unwrap();
    let mut session = metadata_only_session(dir.path());
    let mut encoder: Option<EncodeSession> = None;
    let tap = fill_tap();
    let budget = Duration::from_secs_f64(1.0 / 30.0);

    // Baseline: render accounting with the feed DISABLED.
    let dropped_baseline = state
        .dropped_frames_total
        .load(std::sync::atomic::Ordering::SeqCst);
    for frame in 0..5u64 {
        let report = render.render_frame(frame, Some(budget));
        assert!(report.view_late_by.is_none());
    }
    let dropped_no_feed = state
        .dropped_frames_total
        .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        dropped_baseline, dropped_no_feed,
        "baseline: no drops without the feed"
    );

    // With the feed ENABLED the render deadline accounting must behave
    // identically (the feed runs after the check, on its own counter), while
    // the feed reports its own time separately — including the measured
    // readback the loop folds into the engine-state counter.
    let mut record_tap_ms = 0.0f64;
    for frame in 5..10u64 {
        let report = render.render_frame(frame, Some(budget));
        assert!(report.view_late_by.is_none());
        let readback_started = Instant::now();
        let rgba = render.readback_view().await;
        let readback_elapsed = readback_started.elapsed();
        // Render elapsed as the loop measures it: the feed must never see
        // its own cost folded back into this number.
        let render_elapsed = Duration::from_millis(2);
        let outcome = feed_record_frame(
            &rgba,
            &mut encoder,
            &mut session,
            &tap,
            render_elapsed,
            Some(budget),
            readback_elapsed,
        );
        assert!(!outcome.skipped);
        assert!(
            outcome.feed_ms >= readback_elapsed.as_secs_f64() * 1000.0,
            "feed_ms must include the measured readback"
        );
        record_tap_ms += outcome.feed_ms;
    }
    let dropped_with_feed = state
        .dropped_frames_total
        .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        dropped_no_feed, dropped_with_feed,
        "feed time must never leak into render deadline accounting"
    );
    assert!(
        record_tap_ms > 0.0,
        "separate record_tap_ms counter must accumulate, got {record_tap_ms}"
    );
    // The loop-owned counters live in engine state (updated by the loop, not
    // by direct feed calls — so they read zero here); direct calls prove the
    // fields exist and default honestly.
    assert_eq!(
        *state.record_tap_ms.lock().unwrap(),
        0.0,
        "direct feed calls never touch the loop's state counter"
    );
    assert_eq!(
        state
            .skipped_record_frames
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no skip happened, so the loop's skip counter stays zero"
    );
}
