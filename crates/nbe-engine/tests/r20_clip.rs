//! R20 (docs/soak-protocol.md §5): a plain video clip stops drawing after its
//! first 30 frames. The falsification battery, written before any fix.
//!
//! A clip with no declared loop period is decoded only to
//! `CLIP_PRELOAD_FRAMES` (30) at load, the ring's period falls back to that
//! count, `clip_source_index` returns `None` once the source index reaches it,
//! and `draw_for` drops the layer. No test looked at the picture, so nothing
//! saw it. These tests do: each asks the real draw path for a frame past the
//! ring (`RenderLoop::render_frame`, then `readback_view`), the way Prompt 05's
//! pixel tests do.
//!
//! - The weaker form (the drop-detector), on the dress's A1: frame 45 must be a
//!   drawn layer, not the black a dropped layer leaves. A1 is one repeated
//!   picture, so this passes under a hold-last-frame stopgap and under the fix
//!   alike; it catches the drop.
//! - The stronger form (the stopgap-police), on `counter_60.mp4`: rendered
//!   frame 45 must be the source's own frame 45, decoded independently of the
//!   ring and the draw path. It fails under the drop and under the stopgap.
//! - The fixture-integrity guard: every frame of `counter_60.mp4` is distinct,
//!   so a future fixture edit cannot blind the stronger form.
//!
//! No-hardware policy, as Prompt 05: without VideoToolbox or a wgpu adapter
//! these tests FAIL, naming what was missing. They never skip.

#[path = "common/capture.rs"]
mod capture;

use nbe_engine::decode::DecodeSession;
use nbe_engine::directive::DirectiveHandler;
use nbe_engine::gpu::Gpu;
use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Frame 45: past the 30-frame clip preload, inside both clips.
const PAST_THE_RING: u64 = 45;
/// Frame 15: inside the preload, where today's path draws. The control that
/// makes a failure at 45 attributable to R20 and not to the harness.
const INSIDE_THE_RING: u64 = 15;
/// Per-channel tolerance between a rendered and a decoded pixel. Adjacent
/// frames of `counter_60.mp4` differ by at least 37 in red.
const TOLERANCE: i32 = 10;

/// The suite's lane (the user's ruling of 2026-10-19 on PR #59's gate). The
/// refill guard measures one frame's cost against a bound, so it runs alone:
/// it takes the lane for writing, and every other test here takes it for
/// reading. Under the workspace step's default thread count the guard then
/// never shares the GPU or the decoder with a sibling. Run beside its eight
/// siblings it measured 5–8 ms, and once 9.93 ms, past its bound, at a load of
/// 4.0; run alone, 2.1–2.4 ms. The bound (8.33 ms) is unchanged.
static LANE: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

fn dress_media(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/dress_show/media")
        .join(name)
}

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

/// A house-format package with one plain clip (no loop period) on item `C1`.
fn write_clip_package(dir: &Path, clip: &Path, duration_frames: u64) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    std::fs::copy(clip, dir.join("media/clip.mp4")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
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
                "video": { "width": 1920, "height": 1080, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate"
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" },
                { "id": "clip", "kind": "video", "source": "media/clip.mp4", "format": "h264" }
            ],
            "scenes": [{ "id": "SCN", "elements": [
                { "id": "main", "kind": "clip", "z": 1, "assetId": "clip" }
            ]}],
            "rundown": { "id": "R", "items": [
                { "id": "C1", "kind": "sceneRef", "sceneRef": "SCN", "durationFrames": duration_frames }
            ]},
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();
}

/// Load the package and put `C1` on the View from master frame 0.
async fn on_air(dir: &Path) -> RenderLoop {
    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(OutgoingQueue::default());
    DirectiveHandler::new(state.clone(), outgoing)
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({ "packagePath": dir.to_string_lossy() }),
        ))
        .await
        .unwrap();
    *state.view_item.lock().unwrap() = Some("C1".into());
    RenderLoop::new(state)
        .await
        .expect("R20 needs a wgpu adapter to read the picture back")
}

fn centre(bytes: &[u8], width: u32, height: u32) -> [u8; 3] {
    let idx = (((height / 2) * width + width / 2) * 4) as usize;
    [bytes[idx], bytes[idx + 1], bytes[idx + 2]]
}

async fn rendered_centre(render: &mut RenderLoop, frame: u64) -> [u8; 3] {
    render.render_frame(frame, None);
    centre(&render.readback_view().await, VIEW_W, VIEW_H)
}

/// One source frame, kept small: its centre pixel and a hash of all its
/// pixels. A whole 1080p frame is 7.91 MiB, and the workspace runs these tests
/// in parallel, so only the facts the assertions read are kept.
struct SourceFrame {
    index: u64,
    centre: [u8; 3],
    hash: u64,
}

/// Every frame of a fixture, decoded once per test process and shared: the
/// suite reads each source's truth several times, and decoding it again each
/// time only loads the machine.
fn decoded(name: &str) -> &'static [SourceFrame] {
    static COUNTER_60: std::sync::OnceLock<Vec<SourceFrame>> = std::sync::OnceLock::new();
    static COUNTER_150: std::sync::OnceLock<Vec<SourceFrame>> = std::sync::OnceLock::new();
    let cell = match name {
        "counter_60.mp4" => &COUNTER_60,
        "counter_150.mp4" => &COUNTER_150,
        other => panic!("R20: no shared decode for {other}"),
    };
    cell.get_or_init(|| decode_all(&dress_media(name)))
}

/// Every frame of a source, decoded by `nbe_decode` on its own: no ring, no
/// preload cap, no draw path. One frame is resident at a time.
fn decode_all(path: &Path) -> Vec<SourceFrame> {
    let mut session =
        DecodeSession::open(path).expect("R20 needs VideoToolbox to decode the source");
    let mut frames = Vec::new();
    while let Some(f) = session.next_frame().expect("decode") {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        f.rgba.hash(&mut h);
        frames.push(SourceFrame {
            index: f.index,
            centre: centre(&f.rgba, f.width, f.height),
            hash: h.finish(),
        });
    }
    frames
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| (*x as i32 - *y as i32).abs() <= TOLERANCE)
}

#[tokio::test]
async fn r20_weaker_form_a_clip_still_draws_past_its_first_second() {
    let _lane = LANE.read().await;
    // A1 is 150 frames (5 s), one repeated dark blue picture; the View clears
    // to black, so a dropped layer reads black.
    let dir = tempfile::tempdir().unwrap();
    write_clip_package(dir.path(), &dress_media("A1.mp4"), 150);
    let mut render = on_air(dir.path()).await;

    let inside = rendered_centre(&mut render, INSIDE_THE_RING).await;
    assert!(
        inside != [0, 0, 0],
        "control: frame {INSIDE_THE_RING} is inside the preload and must draw A1, got {inside:?}"
    );
    let past = rendered_centre(&mut render, PAST_THE_RING).await;
    assert!(
        past != [0, 0, 0] && close(past, inside),
        "R20: A1 is 150 frames, but at frame {PAST_THE_RING} the clip's layer was not drawn \
         (centre {past:?}, black is the cleared View; A1 drew {inside:?} at frame {INSIDE_THE_RING})"
    );
}

#[tokio::test]
async fn r20_stronger_form_rendered_frame_45_is_the_sources_own_frame_45() {
    let _lane = LANE.read().await;
    let source = dress_media("counter_60.mp4");
    let decoded = decoded("counter_60.mp4");
    let truth = |n: u64| decoded[n as usize].centre;

    let dir = tempfile::tempdir().unwrap();
    write_clip_package(dir.path(), &source, decoded.len() as u64);
    let mut render = on_air(dir.path()).await;

    let inside = rendered_centre(&mut render, INSIDE_THE_RING).await;
    assert!(
        close(inside, truth(INSIDE_THE_RING)),
        "control: rendered frame {INSIDE_THE_RING} must be the source's frame {INSIDE_THE_RING} \
         {:?}, got {inside:?}",
        truth(INSIDE_THE_RING)
    );
    let past = rendered_centre(&mut render, PAST_THE_RING).await;
    assert!(
        close(past, truth(PAST_THE_RING)),
        "R20: rendered frame {PAST_THE_RING} must be the source's own frame {PAST_THE_RING} {:?}, \
         got {past:?} (black: the layer was dropped; the source's frame 29 {:?}: a held last \
         frame; frame 44 {:?} or 46 {:?}: an off-by-one)",
        truth(PAST_THE_RING),
        truth(29),
        truth(44),
        truth(46)
    );
}

#[test]
fn r20_fixture_integrity_every_counter_frame_is_distinct() {
    let _lane = LANE.blocking_read();
    // The stripper-test lesson: a guard is only as good as its fixture. If
    // `counter_60.mp4` ever goes static, or shorter than frame 45, the stronger
    // form above goes blind; this fails first.
    let frames = decoded("counter_60.mp4");
    assert!(
        frames.len() as u64 > PAST_THE_RING,
        "R20 fixture: counter_60.mp4 must outlive frame {PAST_THE_RING}, decoded {} frames",
        frames.len()
    );
    let mut seen = HashSet::new();
    for f in frames {
        assert!(
            seen.insert(f.hash),
            "R20 fixture: counter_60.mp4 frame {} repeats an earlier frame; every frame must differ",
            f.index
        );
    }
}

// ---------------------------------------------------------------------------
// The fix's guards (SPEC §12.8.1, v0.4.8 row 12). They come with the fix, as
// the design record's falsification plan maps them: M2b, M4, M5 and M6. M1,
// M2 and M3 die to the battery above, and M7 to the integrity guards.
// ---------------------------------------------------------------------------

use std::sync::atomic::Ordering;
use std::time::Duration;

/// How long a test waits for the read-ahead to fill its window. A backstop,
/// not a timing claim: the settle returns as soon as the window is full.
const SETTLE: Duration = Duration::from_secs(20);

fn misses(render: &RenderLoop) -> u64 {
    render.state.read_ahead_misses_total.load(Ordering::SeqCst)
}

#[test]
fn r20_fixture_integrity_every_counter_150_frame_is_distinct() {
    let _lane = LANE.blocking_read();
    // M7 for the streaming fixture: counter_150 must outlive the clip's head
    // plus its window (30 + 60 = 90 frames), or the guards below cannot see
    // the read-ahead stream at all.
    let frames = decoded("counter_150.mp4");
    assert!(
        frames.len() > 90,
        "R20 fixture: counter_150.mp4 must outlive a clip's head and window (90 frames), decoded {}",
        frames.len()
    );
    let mut seen = HashSet::new();
    for f in frames {
        assert!(
            seen.insert(f.hash),
            "R20 fixture: counter_150.mp4 frame {} repeats an earlier frame; every frame must differ",
            f.index
        );
    }
}

#[tokio::test]
async fn r20_a_clip_plays_its_full_length_frame_for_frame() {
    let _lane = LANE.read().await;
    // M2b, the dead worker: with the load-time prime alone a clip would play
    // to frame 89 and hold. counter_150 runs to 149, so frames 90 and later
    // exist only if the read-ahead streams them. Each frame is read back after
    // the window settles at it, so nothing here races the decoder: the
    // occupancy decides, never a clock (§12.8.1 rule 3).
    let source = dress_media("counter_150.mp4");
    let decoded = decoded("counter_150.mp4");
    let dir = tempfile::tempdir().unwrap();
    write_clip_package(dir.path(), &source, decoded.len() as u64);
    let mut render = on_air(dir.path()).await;
    for n in [0u64, 15, 29, 30, 45, 89, 90, 95, 120, 149] {
        assert!(
            render.settle_read_ahead(n, SETTLE),
            "R20: the read-ahead did not fill its window at frame {n}"
        );
        let got = rendered_centre(&mut render, n).await;
        let want = decoded[n as usize].centre;
        assert!(
            close(got, want),
            "R20: rendered frame {n} must be the source's own frame {n} {want:?}, got {got:?}"
        );
    }
    assert_eq!(
        misses(&render),
        0,
        "a settled read-ahead draws every frame resident: no misses"
    );
}

/// R47: rounds of the settle, a count, never a deadline (R39; R48's form).
/// Each round pumps the read-ahead once, uncapped, and reads whether the
/// window is settled; rounds are 2 ms apart, so 15,000 is 30 s or more, and a
/// stall lengthens the wait without ever ending it.
const SETTLE_ROUNDS: u32 = 15_000;
/// R47: polls of the device, 2 ms apart, before a submission is called lost.
const GPU_POLLS: u32 = 15_000;

/// R47: settle the read-ahead at `frame` by count. `settle_read_ahead` given
/// no time of its own is one round: one uncapped pump, then the settled
/// check. Returns the rounds it took, or `None` when every round was spent.
fn settle_by_count(render: &mut RenderLoop, frame: u64) -> Option<u32> {
    for round in 1..=SETTLE_ROUNDS {
        if render.settle_read_ahead(frame, Duration::ZERO) {
            return Some(round);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

/// R47: wait, by gate and count, for the work submitted so far to finish on
/// the GPU. `on_submitted_work_done` opens the gate once the last submission
/// (and so every one before it) has completed; the device is polled, never
/// waited on with a timeout, at most `GPU_POLLS` times. Returns the polls it
/// took, or `None`.
fn gpu_done(gpu: &Gpu) -> Option<u32> {
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let open = gate.clone();
    gpu.queue
        .on_submitted_work_done(move || open.store(true, Ordering::SeqCst));
    for poll in 1..=GPU_POLLS {
        gpu.device
            .poll(wgpu::PollType::Poll)
            .expect("R47: polling the device failed");
        if gate.load(Ordering::SeqCst) {
            return Some(poll);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

/// R47: read a texture back by gate and count: the copy is submitted, its
/// buffer mapped, and the device polled until the map's callback opens the
/// gate, at most `GPU_POLLS` times. Never a discarded timeout: a copy that
/// never completes fails by name.
fn read_back(gpu: &Gpu, tex: &wgpu::Texture) -> Vec<u8> {
    let (width, height) = (tex.width(), tex.height());
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let row_bytes = (width * 4).div_ceil(align) * align;
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("R47 readback"),
        size: row_bytes as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([enc.finish()]);
    let mapped = Arc::new(std::sync::Mutex::new(None));
    let done = mapped.clone();
    buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        *done.lock().unwrap() = Some(r);
    });
    let mut polls = 0;
    let result = loop {
        gpu.device
            .poll(wgpu::PollType::Poll)
            .expect("R47: polling the device failed");
        polls += 1;
        if let Some(r) = mapped.lock().unwrap().take() {
            break r;
        }
        assert!(
            polls < GPU_POLLS,
            "R47: a readback's copy was not mapped after {GPU_POLLS} polls of the device"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    result.expect("R47: the readback's buffer failed to map");
    let data = buf
        .slice(..)
        .get_mapped_range()
        .expect("R47: the readback's mapped range");
    let row_out = (width * 4) as usize;
    let mut out = Vec::with_capacity(row_out * height as usize);
    for row in 0..height as usize {
        let off = row * row_bytes as usize;
        out.extend_from_slice(&data[off..off + row_out]);
    }
    out
}

/// R47: draw frame `n` on the View, wait for the View's own submission and
/// everything before it by gate and count, and read the View back by gate and
/// count. Err names what never came.
fn drawn_centre(render: &mut RenderLoop, n: u64) -> Result<[u8; 3], String> {
    render.render_frame(n, None);
    if render.view_submission().is_none() {
        return Err(format!("R47: frame {n}'s View was not submitted"));
    }
    if gpu_done(&render.gpu).is_none() {
        return Err(format!(
            "R47: frame {n}'s View submission did not complete after {GPU_POLLS} polls of the device"
        ));
    }
    let bytes = read_back(&render.gpu, render.view_target());
    Ok(centre(&bytes, VIEW_W, VIEW_H))
}

/// R47: what the window held for source frame `j`, for a failure's message:
/// resident(j), the slot `j` lives in and the frame that slot holds, the
/// slot's own picture read back (so a slot that held the wrong frame or no
/// picture reads apart from a layer that drew nothing), the miss counter, and
/// the window's state.
fn window_facts(render: &RenderLoop, j: u64) -> String {
    let mut out = vec![format!("read_ahead_misses_total {}", misses(render))];
    for r in render.clip_window_report(j) {
        let own = r
            .slot_texture
            .as_ref()
            .map(|t| centre(&read_back(&render.gpu, t), t.width(), t.height()));
        out.push(format!(
            "clip {}: resident({j}) {}, slot {:?} holds frame {:?}, the slot's own centre {:?}; \
             window {} slots from base {} past a head of {}, first missing {:?}, pending {:?}, \
             worker next {:?}, last presented {:?}",
            r.asset_id,
            r.resident,
            r.slot,
            r.slot_frame,
            own,
            r.window_len,
            r.base,
            r.head,
            r.first_missing,
            r.pending,
            r.worker_next,
            r.last_presented
        ));
    }
    out.join("; ")
}

#[tokio::test]
async fn r20_the_window_never_overwrites_a_frame_the_draw_still_needs() {
    let _lane = LANE.read().await;
    // M4, the occupancy miscount. ~~At frame 15 the window must hold [30, 90).~~
    // (Struck: the window [30, 90) is primed at load, so at frame 15 no
    // decoder runs and nothing could overwrite it. M4 survived that form.)
    // At frame 45 the window is [45, 105): frames 45..89 come from the prime,
    // and a decoder runs from 90 to fill 90..104. Frame 105 belongs in frame
    // 45's own slot, so a correct read-ahead holds 105 back until the draw
    // passes 45 (rule 3); one that wrote past its window would put 105 there,
    // and frame 45 would read as frame 105.
    //
    // R47 (docs/soak-protocol.md §5): the settle is a count of rounds, never a
    // clock; every draw is waited for on the GPU by gate and count, and read
    // back the same way, never through a discarded timeout; a mismatch names
    // the window's facts (resident(45), slot 15's frame and its own picture,
    // the miss counter); and the engine's log is captured through
    // tests/common/capture.rs, witnessed by the load's own line.
    let source = dress_media("counter_150.mp4");
    let decoded = decoded("counter_150.mp4");
    let truth = |n: u64| decoded[n as usize].centre;
    let dir = tempfile::tempdir().unwrap();
    write_clip_package(dir.path(), &source, decoded.len() as u64);
    let (outcome, log) = capture::captured(async {
        let mut render = on_air(dir.path()).await;
        drawn_centre(&mut render, INSIDE_THE_RING)?;
        drawn_centre(&mut render, PAST_THE_RING)?;
        if settle_by_count(&mut render, PAST_THE_RING).is_none() {
            return Err(format!(
                "R20: at frame {PAST_THE_RING} the read-ahead must fill its window [45, 105) and hold \
                 its decoder at the window's end (occupancy, §12.8.1 rule 3); it did not settle in \
                 {SETTLE_ROUNDS} rounds. R47: {}",
                window_facts(&render, PAST_THE_RING)
            ));
        }
        for n in [PAST_THE_RING, 89, 104] {
            let got = drawn_centre(&mut render, n)?;
            if !close(got, truth(n)) {
                return Err(format!(
                    "R20: with the window full at [45, 105), frame {n} must be {:?}, got {got:?} \
                     (frame 105 {:?}: a slot overwritten past the window). R47: {}",
                    truth(n),
                    truth(105),
                    window_facts(&render, n)
                ));
            }
        }
        Ok::<(), String>(())
    })
    .await;
    assert!(
        log.contains("clip ring resident"),
        "R47: the capture saw nothing of the load (no \"clip ring resident\" line), so the log \
         below proves nothing: {log}"
    );
    if let Err(e) = outcome {
        panic!("{e}\nthe engine's log on this thread:\n{log}");
    }
}

/// The frame deadline at the house rate, 30 fps (SPEC §7.13).
const FRAME_DEADLINE: Duration = Duration::from_micros(33_333);
/// The read-ahead's upload cap in one frame (`READ_AHEAD_UPLOADS_PER_FRAME`).
const UPLOADS_AT_THE_CAP: u32 = 4;

#[tokio::test]
async fn r20_a_window_refill_fits_the_frame_deadline() {
    let _lane = LANE.write().await;
    // M8, the frame path back on `write_texture`. The refill runs in the frame
    // path, inside the record's pre-check (a frame whose `render_frame`
    // overran is not recorded) but outside the View's drop timer, so no drop
    // counter sees it. At the cap, four uploads must fit one frame's deadline:
    // each must cost under a quarter of it, 8.33 ms. `write_texture` creates a
    // staging buffer per call and costs 10.5 ms a 1080p frame on the
    // reference machine; four of them made 52 ms frames, and the gate's takes
    // recorded nothing. Through the staging pool a frame costs about 1 ms.
    //
    // Each measured frame uploads exactly one window frame: settled at n, the
    // window holds [n, n + 60) and the decoder holds n + 60 back; drawing
    // n + 1 frees n's slot, and n + 60 goes into it.
    let source = dress_media("counter_150.mp4");
    let decoded = decoded("counter_150.mp4");
    let dir = tempfile::tempdir().unwrap();
    write_clip_package(dir.path(), &source, decoded.len() as u64);
    let mut render = on_air(dir.path()).await;
    let mut costs = Vec::new();
    for n in PAST_THE_RING..PAST_THE_RING + 10 {
        assert!(
            render.settle_read_ahead(n, SETTLE),
            "R20: the read-ahead did not settle at {n}"
        );
        let report = render.render_frame(n + 1, None);
        assert_eq!(
            report.read_ahead_uploads,
            1,
            "R20: settled at {n}, drawing {} uploads frame {} and nothing else",
            n + 1,
            n + 60
        );
        costs.push(report.read_ahead);
    }
    costs.sort();
    let median = costs[costs.len() / 2];
    let bound = FRAME_DEADLINE / UPLOADS_AT_THE_CAP;
    println!("R20 REFILL: one window upload, median {median:?} of {costs:?}, bound {bound:?}");
    assert!(
        median < bound,
        "R20: one window upload must cost under {bound:?}, a quarter of the frame deadline, so the \
         read-ahead's cap fits one frame; the median of {} was {median:?} (all: {costs:?})",
        costs.len()
    );
}

#[tokio::test]
async fn r20_a_miss_holds_the_last_frame_counts_it_and_never_blocks() {
    let _lane = LANE.read().await;
    // M5. A jump from frame 15 to frame 120 outruns any decoder: 120 is not in
    // the window [30, 90), and the read-ahead restarts there. The View must
    // hold the last frame it drew (15), never black and never a wait that
    // would draw 120 late; the miss is counted on the tick's counter; and once
    // the window settles at 120 the clip plays on.
    let source = dress_media("counter_150.mp4");
    let decoded = decoded("counter_150.mp4");
    let truth = |n: u64| decoded[n as usize].centre;
    let dir = tempfile::tempdir().unwrap();
    write_clip_package(dir.path(), &source, decoded.len() as u64);
    let mut render = on_air(dir.path()).await;
    rendered_centre(&mut render, INSIDE_THE_RING).await;
    let before = misses(&render);
    let held = rendered_centre(&mut render, 120).await;
    assert!(
        close(held, truth(INSIDE_THE_RING)),
        "R20: a miss at frame 120 must hold the last frame drawn, frame {INSIDE_THE_RING} {:?}, \
         got {held:?} (black: drawn black; frame 120 {:?}: the draw waited on the decoder)",
        truth(INSIDE_THE_RING),
        truth(120)
    );
    assert_eq!(misses(&render), before + 1, "R20: the miss is counted once");
    assert!(
        render.settle_read_ahead(120, SETTLE),
        "R20: the read-ahead recovers at 120"
    );
    let after = rendered_centre(&mut render, 120).await;
    assert!(
        close(after, truth(120)),
        "R20: once the window settles at 120 the clip plays on, got {after:?}"
    );
}

#[tokio::test]
async fn r20_past_its_end_a_clip_draws_nothing() {
    let _lane = LANE.read().await;
    // M6. The clip's end is its metadata's length (§12.8.1 rule 5): frame 59
    // is counter_60's last, and from frame 60 the clip has ended and its layer
    // is not drawn. A clip that held past its end would show frame 59 there.
    let source = dress_media("counter_60.mp4");
    let decoded = decoded("counter_60.mp4");
    let last = { decoded[59].centre };
    let dir = tempfile::tempdir().unwrap();
    write_clip_package(dir.path(), &source, 90);
    let mut render = on_air(dir.path()).await;
    assert!(
        render.settle_read_ahead(59, SETTLE),
        "R20: the read-ahead did not settle at 59"
    );
    let at_last = rendered_centre(&mut render, 59).await;
    assert!(
        close(at_last, last),
        "R20: frame 59 is the clip's last, {last:?}; got {at_last:?}"
    );
    for n in [60u64, 75] {
        let got = rendered_centre(&mut render, n).await;
        assert_eq!(
            got,
            [0, 0, 0],
            "R20: at frame {n} counter_60 has ended and its layer must not be drawn \
             (frame 59 {last:?}: held past its end), got {got:?}"
        );
    }
}
