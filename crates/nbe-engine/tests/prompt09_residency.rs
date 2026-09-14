//! WU6 [RI-8] unload-at-next-load: show.stop releases decode sessions
//! but retains package residency until next show.load replaces it.

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

fn write_package(dir: &Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    // av_tone carries a real audio track so audio_assets is resident.
    std::fs::copy(media("av_tone.mp4"), dir.join("media/clip.mp4")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();
    std::fs::write(dir.join("media/bg.png"), &png).unwrap();

    std::fs::write(
        dir.join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" },
                { "id": "bg", "kind": "image", "source": "media/bg.png", "format": "png" },
                { "id": "clip", "kind": "video", "source": "media/clip.mp4", "format": "h264" }
            ],
            "scenes": [{ "id": "SCN", "elements": [
                { "id": "main", "kind": "clip", "z": 1, "assetId": "clip" }
            ]}],
            "rundown": { "id": "R", "items": [
                { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN" }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

async fn load(dir: &Path, sv: u64) -> (Arc<EngineState>, Arc<OutgoingQueue>, DirectiveHandler) {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing.clone());
    handler
        .apply(&directive(
            "show.load",
            sv,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.to_string_lossy() }),
        ))
        .await
        .unwrap();
    (state, outgoing, handler)
}

fn decode_sessions(state: &EngineState) -> u32 {
    // Active semantics: telemetry reports live sessions, so a zero here
    // means no session is actually held — not a zeroed peak beside a leak.
    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } =
        nbe_engine::telemetry::build_tick(state)
    else {
        panic!("expected EngineTelemetry");
    };
    let via_telemetry = fields.decode_sessions;
    assert_eq!(
        via_telemetry,
        state.sessions.active(),
        "telemetry decodeSessions must report active sessions"
    );
    via_telemetry
}

#[tokio::test]
async fn show_stop_releases_decode_sessions_but_retains_residency() {
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path());
    let (state, _out, handler) = load(dir.path(), 1).await;

    // Residency held after load: a decode session, a video ring, an image,
    // and audio assets.
    assert!(
        decode_sessions(&state) >= 1,
        "load must hold a decode session, got {}",
        decode_sessions(&state)
    );
    assert!(
        state.video.lock().unwrap().assets.contains_key("clip"),
        "video ring must be resident after load"
    );

    handler
        .apply(&directive(
            "show.stop",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    // Grace window: decodeSessions == 0.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if decode_sessions(&state) == 0 {
            break;
        }
        if Instant::now() > deadline {
            panic!(
                "show.stop must release decode sessions within grace window, still {}",
                decode_sessions(&state)
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Residency retained: rings, textures, audio assets survive stop.
    assert!(
        state.video.lock().unwrap().assets.contains_key("clip"),
        "video ring must survive show.stop (unload-at-next-load)"
    );
    let pkg = state.package.lock().unwrap();
    assert!(
        pkg.as_ref().is_some_and(|p| p.images.contains_key("bg")),
        "image texture must survive show.stop"
    );
    drop(pkg);
    assert!(
        !state.audio_assets.lock().unwrap().is_empty(),
        "audio assets must survive show.stop"
    );
}

#[tokio::test]
async fn second_load_after_stop_rebuilds_without_stacking() {
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path());
    let (state, _out, handler) = load(dir.path(), 1).await;
    // Honest counts, not peak comparisons: one video asset holds exactly one
    // live session, and the library holds exactly one ring.
    assert_eq!(
        state.sessions.active(),
        1,
        "first load must hold exactly one live session"
    );
    assert_eq!(
        state.video.lock().unwrap().assets.len(),
        1,
        "first load must resident exactly one video ring"
    );

    handler
        .apply(&directive(
            "show.stop",
            2,
            serde_json::json!({}),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(
        state.sessions.active(),
        0,
        "stop must release held sessions"
    );

    handler
        .apply(&directive(
            "show.load",
            3,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    // Rebuild, not accumulate: same live count, same library size, and no
    // refusal — the cap was never touched.
    assert_eq!(
        state.sessions.active(),
        1,
        "second load must hold exactly one live session, not stack"
    );
    assert_eq!(
        state.video.lock().unwrap().assets.len(),
        1,
        "second load must resident exactly one video ring"
    );
    assert_eq!(
        state.sessions.refused(),
        0,
        "stop+reload must not refuse any session"
    );
}

#[tokio::test]
async fn load_load_without_stop_does_not_stack_sessions() {
    let dir = tempfile::tempdir().unwrap();
    write_package(dir.path());
    let (state, _out, handler) = load(dir.path(), 1).await;
    assert_eq!(state.sessions.active(), 1);

    // No stop in between: release-then-rebuild at the top of the second load
    // must keep the live count bounded instead of stacking to the cap.
    handler
        .apply(&directive(
            "show.load",
            2,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    assert_eq!(
        state.sessions.active(),
        1,
        "load→load with no stop must not stack sessions"
    );
    assert_eq!(
        state.video.lock().unwrap().assets.len(),
        1,
        "load→load must replace the library, not grow it"
    );
    assert_eq!(
        state.sessions.refused(),
        0,
        "load→load must not spuriously refuse sessions"
    );
}
