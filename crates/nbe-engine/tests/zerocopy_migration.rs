//! ZERO-COPY Phase 3b, step 5 — THE MIGRATION, end to end through real
//! commands.
//!
//! This is the first point at which telemetry's claim and the frame path's
//! behaviour are the same statement, so these are the two falsifications the
//! work order named:
//!
//! 1. **probe-false → `cpuReadback` + `ProbeUnavailable`**, and the take still
//!    passes every structure assertion. The v0.4.2 allowance is what makes that
//!    take lawful; a machine with no chain must still record.
//! 2. **a zero-copy take → `zeroCopy` + `Table`**, with the same structure
//!    assertions unchanged, and with the readback NEVER awaited — that removal
//!    is what the whole migration is for.
//!
//! Every frame goes through `begin_tap_frame` → `render_frame` →
//! `restore_view` → `end_tap_frame`, which is what `main.rs` calls, in that
//! order. Nothing here writes a selection or a counter by hand (§2a rule 7).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::record::AudioTap;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};

/// One take at a time: the hardware encoder is a shared resource.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

fn hw_or_skip() -> bool {
    if nbe_engine::encode::is_available() {
        return true;
    }
    eprintln!("SKIP: no hardware H.264 encoder on this machine (SPEC §9.2); recording has no CPU fallback");
    false
}

fn aac_or_skip() -> bool {
    if nbe_engine::record::aac::is_available() {
        return true;
    }
    eprintln!("SKIP: AudioToolbox AAC absent — a take needs the audio encoder");
    false
}

fn ffprobe_or_skip() -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from("/usr/local/bin/ffprobe");
    if p.is_file() {
        return Some(p);
    }
    eprintln!("SKIP: /usr/local/bin/ffprobe absent — the structure assertions need it");
    None
}

fn ffprobe_streams(ffprobe: &Path, file: &Path) -> serde_json::Value {
    let out = std::process::Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(file)
        .output()
        .expect("spawning ffprobe must succeed");
    assert!(
        out.status.success(),
        "ffprobe must parse the recording, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("ffprobe JSON must parse")
}

/// 440 Hz stereo tone, amplitude 0.5, interleaved f32.
fn tone_secs(secs: u32) -> Vec<f32> {
    let frames = (48_000 * secs) as usize;
    let mut pcm = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48_000.0).sin();
        pcm.push(v);
        pcm.push(v);
    }
    pcm
}

/// The path telemetry reports, read out of a REAL `build_tick`.
fn reported_path(state: &Arc<EngineState>) -> (String, String) {
    let frame = nbe_engine::telemetry::build_tick(state);
    let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } = frame else {
        panic!("build_tick must produce telemetry");
    };
    (fields.record_tap_path, fields.record_tap_reason)
}

/// Run one take of `frames` frames through the loop's real seams, in the loop's
/// real order, and return the output path plus what happened.
struct TakeOutcome {
    path: std::path::PathBuf,
    readback_awaited: bool,
    tap_ms: f64,
    skipped: u64,
}

async fn run_take(
    state: &Arc<EngineState>,
    handler: &DirectiveHandler,
    render: &mut RenderLoop,
    frames: u32,
) -> TakeOutcome {
    let dir = Box::leak(Box::new(tempfile::tempdir().expect("tempdir")));
    *state.record_dir.lock().unwrap() = Some(dir.path().to_path_buf());
    handler
        .apply(&directive("show.start", 1, serde_json::json!({})))
        .await
        .unwrap();
    handler
        .apply(&directive(
            "record.start",
            2,
            serde_json::json!({"outputId": "ep01"}),
        ))
        .await
        .expect("record.start on a RUNNING show must open the pipeline");

    let path = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .expect("session")
        .output_path()
        .to_path_buf();
    let tap: Arc<AudioTap> = state.record_session.lock().unwrap().as_ref().unwrap().tap();
    tap.push(&tone_secs(2));

    let budget = Duration::from_secs_f64(1.0 / 30.0);
    let readback_awaited = AtomicBool::new(false);
    let skipped = &state.skipped_record_frames;
    // PACED to the frame boundary, exactly as `main.rs` paces. Without this the
    // feed runs as fast as the CPU allows, the bounded channel fills on frame 2
    // and 33 of 40 frames shed — an artifact of the harness, not of the system,
    // and one that hid the real numbers on the first run of this suite.
    let mut next_boundary = Instant::now() + budget;
    for frame in 0..frames {
        // The loop's order, exactly: resolve pool + endpoint under one lock,
        // acquire and retarget BEFORE the draw, restore immediately after,
        // then skip or hand off.
        let (pool, endpoint) = {
            let g = state.record_session.lock().unwrap();
            match g.as_ref() {
                Some(s) => (s.surface_pool(), Some(s.frame_sender())),
                None => (None, None),
            }
        };
        let loan = nbe_engine::record::begin_tap_frame(render, pool.as_deref(), skipped)
            .expect("retarget");
        let started = Instant::now();
        let _ = render.render_frame(frame as u64, Some(budget));
        let render_elapsed = started.elapsed();
        nbe_engine::record::restore_view(render, &loan);
        let tx = endpoint.expect("a live take has a frame sender");
        let feed_ms = nbe_engine::record::end_tap_frame(
            loan,
            // Well inside budget, so the budget pre-check never fires and the
            // only skips these takes can record are pool exhaustion or a shed.
            Duration::from_millis(2),
            Some(budget),
            &tx,
            skipped,
            || async {
                readback_awaited.store(true, Ordering::SeqCst);
                let t = Instant::now();
                let rgba = render.readback_view().await;
                (rgba, t.elapsed())
            },
        )
        .await;
        let _ = render_elapsed;
        *state.record_tap_ms.lock().unwrap() += feed_ms;
        let now = Instant::now();
        if next_boundary > now {
            tokio::time::sleep(next_boundary - now).await;
        }
        next_boundary += budget;
    }

    handler
        .apply(&directive("record.stop", 3, serde_json::json!({})))
        .await
        .expect("record.stop must finish the pipeline");

    TakeOutcome {
        path,
        readback_awaited: readback_awaited.load(Ordering::SeqCst),
        tap_ms: *state.record_tap_ms.lock().unwrap(),
        skipped: skipped.load(Ordering::SeqCst),
    }
}

// ---------------------------------------------------------------------------
// Falsification 1 — no chain: cpuReadback, reported, and the file is still good
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_machine_with_no_chain_records_by_readback_and_says_so() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() || !aac_or_skip() {
        return;
    }
    let Some(ffprobe) = ffprobe_or_skip() else {
        return;
    };
    nbe_engine::record::markers::clear();
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    let Ok(mut render) = RenderLoop::new(state.clone()).await else {
        eprintln!("SKIP: no wgpu adapter on this machine; a take needs a compositor");
        return;
    };
    // Stands in for a machine with no zero-copy chain — which is how a
    // GPU-less or non-Apple host arrives at `record.start`. The behaviour under
    // test is what the take then DOES; this is its precondition.
    *state.render_device.lock().unwrap() = None;

    assert_eq!(
        reported_path(&state),
        ("none".into(), "none".into()),
        "before the take, the §10.1.1 stub"
    );
    let out = run_take(&state, &handler, &mut render, 40).await;

    assert_eq!(
        reported_path(&state),
        ("cpuReadback".into(), "ProbeUnavailable".into()),
        "a fallback must name itself, or an operator cannot tell it from a choice"
    );
    assert!(
        out.readback_awaited,
        "the CPU path IS the readback; not awaiting it would mean the take \
         recorded something else"
    );
    assert!(
        out.tap_ms > 0.0,
        "the readback's cost lands on record_tap_ms"
    );

    // NO THRESHOLD HERE, deliberately. Paced at 30 fps this take sheds roughly
    // half its frames on the reference machine — the readback is ~12 ms, the
    // copy is 8.3 MiB, and the two together do not reliably fit a 33.3 ms
    // budget alongside the encode. That is the finding the migration exists
    // for, and it is a NUMBER, so it belongs to the soak and not to a test that
    // runs on whatever machine is handy (`docs/soak-protocol.md`: structure
    // gates in CI, thresholds gate on the soak). `prompt09_feed`'s live take
    // never saw it because it feeds synthetic RGBA and never reads back.
    //
    // What IS asserted is the allowance's promise: frames get through, and the
    // file is correct.
    eprintln!(
        "OBSERVED cpuReadback take: {} of {} frames shed, record_tap_ms {:.1}",
        out.skipped, 40, out.tap_ms
    );
    assert!(
        out.skipped < 40,
        "every frame shed: the readback take recorded nothing at all"
    );

    // Every structure assertion, unchanged from `prompt09_feed`'s live take.
    let v = ffprobe_streams(&ffprobe, &out.path);
    let streams = v["streams"].as_array().expect("streams array");
    assert_eq!(streams.len(), 2, "exactly 1 video + 1 audio stream");
    assert!(streams.iter().any(|s| s["codec_name"] == "h264"));
    assert!(streams.iter().any(|s| s["codec_name"] == "aac"));
    nbe_engine::record::markers::clear();
}

// ---------------------------------------------------------------------------
// Falsification 2 — the zero-copy take: zeroCopy/Table, no readback, same file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_zero_copy_take_reports_the_table_and_never_reads_back() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() || !aac_or_skip() {
        return;
    }
    let Some(ffprobe) = ffprobe_or_skip() else {
        return;
    };
    nbe_engine::record::markers::clear();
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    let Ok(mut render) = RenderLoop::new(state.clone()).await else {
        eprintln!("SKIP: no wgpu adapter on this machine; the zero-copy chain needs one");
        return;
    };
    if state.render_device().is_none() {
        eprintln!("SKIP: no device published; the zero-copy chain needs one");
        return;
    }

    let out = run_take(&state, &handler, &mut render, 40).await;

    assert_eq!(
        reported_path(&state),
        ("zeroCopy".into(), "Table".into()),
        "with the chain available the published table chooses, and says it did"
    );
    assert!(
        !out.readback_awaited,
        "THE MIGRATION'S WHOLE POINT: no frame on this path may await a View \
         readback. One await here and the take is the CPU path wearing the \
         zero-copy label"
    );
    assert!(
        out.tap_ms > 0.0,
        "the handoff still costs something and still lands on record_tap_ms"
    );

    // The same structure assertions as the readback take above. A different
    // transport must not produce a different file.
    let v = ffprobe_streams(&ffprobe, &out.path);
    let streams = v["streams"].as_array().expect("streams array");
    assert_eq!(streams.len(), 2, "exactly 1 video + 1 audio stream");
    assert!(streams.iter().any(|s| s["codec_name"] == "h264"));
    assert!(streams.iter().any(|s| s["codec_name"] == "aac"));
    let video = streams
        .iter()
        .find(|s| s["codec_name"] == "h264")
        .expect("h264");
    assert_eq!(video["width"], VIEW_W, "the take's geometry is the View's");
    assert_eq!(video["height"], VIEW_H);
    nbe_engine::record::markers::clear();
}

// ---------------------------------------------------------------------------
// The take owns its pool, and the pool dies with it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_pool_is_built_at_start_sized_to_the_channel_and_gone_after_stop() {
    let _serial = SERIAL.lock().await;
    if !hw_or_skip() {
        return;
    }
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    let Ok(_render) = RenderLoop::new(state.clone()).await else {
        eprintln!("SKIP: no wgpu adapter on this machine; the zero-copy chain needs one");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    *state.record_dir.lock().unwrap() = Some(dir.path().to_path_buf());
    handler
        .apply(&directive("show.start", 1, serde_json::json!({})))
        .await
        .unwrap();
    handler
        .apply(&directive("record.start", 2, serde_json::json!({})))
        .await
        .expect("start");

    let pool = state
        .record_session
        .lock()
        .unwrap()
        .as_ref()
        .expect("session")
        .surface_pool()
        .expect("a zero-copy take has a pool");
    assert_eq!(
        pool.len(),
        nbe_engine::record::RECORD_CHANNEL_BOUND + 1,
        "one surface in flight per channel slot, plus the one being drawn"
    );
    assert_eq!(
        pool.dimensions(),
        (VIEW_W, VIEW_H),
        "the pool is the View's geometry, which is what makes the retarget legal"
    );
    drop(pool);

    // An empty take fails loudly at finish (no video units) — that is existing
    // behaviour and not what this test is about. Either way the session is
    // taken and dropped, and with it the pool.
    let _ = handler
        .apply(&directive("record.stop", 3, serde_json::json!({})))
        .await;
    assert!(
        state.record_session.lock().unwrap().is_none(),
        "the stop takes the session, so the pool's ~25 MiB of VRAM goes with it"
    );
}

/// Source assertion, in the spirit of `prompt09_feed`'s `loop_wiring_*`: the
/// loop must ask for a surface BEFORE it draws. A `begin_tap_frame` call that
/// drifted below `render_frame` would compile, pass every unit test, and
/// corrupt frames in production.
#[test]
fn loop_wiring_acquires_the_surface_before_the_draw() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("main.rs is readable");
    let begin = src
        .find("begin_tap_frame")
        .expect("the loop acquires a surface");
    let draw = src.find("render.render_frame(").expect("the loop draws");
    let restore = src
        .find("restore_view")
        .expect("the loop restores the View");
    let end = src
        .find("end_tap_frame")
        .expect("the loop ends the tap frame");
    assert!(
        begin < draw,
        "begin_tap_frame must precede the draw: on the zero-copy path the draw \
         goes INTO the surface, so asking afterwards costs a corrupted frame \
         rather than a skipped one"
    );
    assert!(
        draw < restore,
        "the View is restored after the draw, not before"
    );
    assert!(restore < end, "restore_view runs before the handoff");
}

/// Companion to the above: the counter the acquire feeds is the one the loop
/// and the thread already share, so a soak's span counters stay comparable
/// across paths.
#[test]
fn the_pool_skip_uses_the_same_counter_as_every_other_skip() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/record/feed.rs"))
        .expect("feed.rs is readable");
    let acquire = src
        .find("pub fn acquire_record_surface")
        .expect("the free-surface question exists");
    let tail = &src[acquire..];
    let body_end = tail.find("\n}\n").expect("the function ends");
    assert!(
        tail[..body_end].contains("skipped.fetch_add(1"),
        "acquire_record_surface must count its refusal, or a zero-copy take \
         under pressure looks like a take with nothing to report"
    );
}
