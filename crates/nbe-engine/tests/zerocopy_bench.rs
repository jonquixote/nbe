//! ZERO-COPY Phase 3b, step 7 — the before/after measurement.
//!
//! `#[ignore]`d: it takes ~25 s, it needs a quiescent machine, and a number
//! measured under load is worse than no number — it is a number someone will
//! later cite (`docs/soak-protocol.md` §2). Run it deliberately:
//!
//! ```text
//! cargo test -p nbe-engine --test zerocopy_bench -- --ignored --nocapture
//! ```
//!
//! **What is timed.** Two spans per frame, reported separately because they
//! answer different questions:
//!
//! - `tap` — what the record path costs: the readback + handoff on the CPU
//!   path, the acquire + retarget + handoff on the zero-copy path. This is the
//!   quantity that lands on `record_tap_ms`, and the one the migration set out
//!   to reduce.
//! - `frame` — render + tap, the whole per-frame cost, against the 33.333 ms
//!   budget. Comparable to the Phase 1 table, which timed "render one frame
//!   into the shared surface, then hand the same surface to the encoder".
//!
//! Unpaced on purpose: pacing to the frame boundary measures the pacing.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::render::{RenderLoop, VIEW_H, VIEW_W};
use nbe_engine::state::EngineState;

const FRAMES: usize = 300;
const BUDGET_MS: f64 = 33.333;
/// The soak protocol's quiescence ceiling. A run above it is VOID, not data.
const LOAD_CEILING: f64 = 3.0;

struct Stats {
    n: usize,
    mean: f64,
    min: f64,
    p50: f64,
    p95: f64,
    max: f64,
    over: usize,
}

fn stats(mut xs: Vec<f64>, over_ms: f64) -> Stats {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = xs.len();
    Stats {
        n,
        mean: xs.iter().sum::<f64>() / n as f64,
        min: xs[0],
        p50: xs[n / 2],
        p95: xs[(n as f64 * 0.95) as usize],
        max: xs[n - 1],
        over: xs.iter().filter(|x| **x > over_ms).count(),
    }
}

fn row(label: &str, s: &Stats) {
    println!(
        "| {label} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {} / {} |",
        s.n, s.mean, s.min, s.p50, s.p95, s.max, s.over, s.n
    );
}

/// 1-minute load average, so the run states its own conditions.
fn load_1m() -> f64 {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .expect("sysctl must run");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_matches(|c| c == '{' || c == '}' || c == ' ')
        .split_whitespace()
        .next()
        .expect("a load average")
        .parse()
        .expect("a number")
}

#[tokio::test]
#[ignore = "measurement: ~25 s, needs a quiescent machine"]
async fn before_and_after_the_migration() {
    let load_before = load_1m();
    println!("load(1m) at start: {load_before:.2}  (ceiling {LOAD_CEILING})");
    assert!(
        load_before <= LOAD_CEILING,
        "VOID: load {load_before:.2} exceeds the {LOAD_CEILING} ceiling; a \
         threshold measured under load is a number someone will later cite"
    );

    let state = Arc::new(EngineState::new(30));
    let mut render = RenderLoop::new(state.clone()).await.expect("a compositor");
    let device = state.render_device().expect("published by RenderLoop::new");

    // A channel deep enough that the encoder's queue is not what is being
    // measured: this bench times the TAP, not the encoder's throughput.
    let (tx, rx) = std::sync::mpsc::sync_channel(FRAMES + 8);
    let drain = std::thread::spawn(move || {
        let mut n = 0usize;
        while rx.recv().is_ok() {
            n += 1;
        }
        n
    });
    let skipped = AtomicU64::new(0);

    // ---- CPU readback, 1080p ------------------------------------------------
    let mut cpu_tap = Vec::with_capacity(FRAMES);
    let mut cpu_frame = Vec::with_capacity(FRAMES);
    for frame in 0..FRAMES {
        let f0 = Instant::now();
        let loan = nbe_engine::record::begin_tap_frame(&mut render, None, false, &skipped)
            .expect("the CPU path has no surface to fail on");
        let _ = render.render_frame(frame as u64, Some(Duration::from_secs_f64(1.0 / 30.0)));
        nbe_engine::record::restore_view(&mut render, &loan);
        let t0 = Instant::now();
        let ms = nbe_engine::record::end_tap_frame(
            loan,
            Duration::from_millis(2),
            Some(Duration::from_secs_f64(1.0 / 30.0)),
            &tx,
            &skipped,
            || async {
                let t = Instant::now();
                let rgba = render.readback_view().await;
                (rgba, t.elapsed())
            },
        )
        .await;
        let tap_wall = t0.elapsed().as_secs_f64() * 1000.0;
        cpu_tap.push(tap_wall);
        cpu_frame.push(f0.elapsed().as_secs_f64() * 1000.0);
        // Two independent derivations of the same quantity: the wall clock
        // around the seam, and what the seam itself reported into
        // `record_tap_ms`. They must agree, or one of them is measuring
        // something else.
        assert!(
            (tap_wall - ms).abs() < 2.0 || ms == 0.0,
            "frame {frame}: wall {tap_wall:.3} ms vs reported {ms:.3} ms"
        );
    }

    // ---- Zero-copy, 1080p ---------------------------------------------------
    let pool = Arc::new(
        nbe_engine::record::zerocopy_pool(&device, VIEW_W, VIEW_H).expect("a pool at View size"),
    );
    let mut zc_tap = Vec::with_capacity(FRAMES);
    let mut zc_frame = Vec::with_capacity(FRAMES);
    let mut zc_pool_skips = 0u64;
    for frame in 0..FRAMES {
        let before = skipped.load(std::sync::atomic::Ordering::SeqCst);
        let f0 = Instant::now();
        let loan = nbe_engine::record::begin_tap_frame(&mut render, Some(&pool), true, &skipped)
            .expect("the pool is at the View's geometry");
        let _ = render.render_frame(frame as u64, Some(Duration::from_secs_f64(1.0 / 30.0)));
        nbe_engine::record::restore_view(&mut render, &loan);
        let t0 = Instant::now();
        let _ms = nbe_engine::record::end_tap_frame(
            loan,
            Duration::from_millis(2),
            Some(Duration::from_secs_f64(1.0 / 30.0)),
            &tx,
            &skipped,
            || async { panic!("the zero-copy path must never read back") },
        )
        .await;
        zc_tap.push(t0.elapsed().as_secs_f64() * 1000.0);
        zc_frame.push(f0.elapsed().as_secs_f64() * 1000.0);
        zc_pool_skips += skipped.load(std::sync::atomic::Ordering::SeqCst) - before;
    }
    drop(tx);
    let drained = drain.join().expect("the drain thread");

    println!("\n== 1080p30, {FRAMES} frames per path, unpaced ==\n");
    println!("| span | n | mean | min | p50 | p95 | max | over 33.333 ms |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    row("cpuReadback tap", &stats(cpu_tap, BUDGET_MS));
    row("zeroCopy tap", &stats(zc_tap, BUDGET_MS));
    row(
        "cpuReadback frame (render+tap)",
        &stats(cpu_frame, BUDGET_MS),
    );
    row("zeroCopy frame (render+tap)", &stats(zc_frame, BUDGET_MS));
    println!(
        "\nframes handed to the drain: {drained}  (requested {}, pool skips {zc_pool_skips})",
        FRAMES * 2
    );
    println!("load(1m) at end: {:.2}", load_1m());
}

/// The 4K row, which is the trip-wire the selection table's second record row
/// exists for. Not through `RenderLoop`: the engine's View is fixed at
/// `VIEW_W x VIEW_H`, so this renders into a 4K surface directly — the same
/// chain, one link short of the production loop, and said so rather than
/// implied.
#[test]
#[ignore = "measurement: needs a quiescent machine"]
fn zero_copy_at_4k() {
    let load = load_1m();
    println!("load(1m): {load:.2}");
    assert!(load <= LOAD_CEILING, "VOID: load {load:.2} over ceiling");

    let inst = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(inst.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("an adapter");
    println!("adapter: {}", adapter.get_info().name);
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("a device");

    let pool = nbe_decode::zerocopy::SurfacePool::new(&device, 3840, 2160, 3).expect("4K pool");
    let mut session =
        nbe_engine::encode::EncodeSession::open(3840, 2160, 30, 8_000_000).expect("4K encoder");

    let mut spans = Vec::with_capacity(FRAMES);
    let mut units = 0usize;
    for _ in 0..FRAMES {
        let surface = pool
            .acquire()
            .expect("a pool of 3 with no consumer is free");
        let t0 = Instant::now();
        // Render into it, as the compositor would.
        let view = surface
            .texture()
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("4k-bench"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        let idx = queue.submit([enc.finish()]);
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: Some(idx),
            timeout: Some(Duration::from_secs(5)),
        });
        // And hand the SAME surface to the encoder.
        units += session
            .encode_pixel_buffer(surface.pixel_buffer())
            .expect("the encoder takes the compositor's allocation")
            .len();
        spans.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let tail = session.finish().expect("the stream finishes").len();

    println!("\n== 4K (3840x2160), {FRAMES} frames, zero-copy, unpaced ==\n");
    println!("| span | n | mean | min | p50 | p95 | max | over 33.333 ms |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    row("zeroCopy render+encode", &stats(spans, BUDGET_MS));
    // Counts two independent ways: units seen streaming, and the total the
    // encoder reports at finish. The second includes the first.
    println!("\nunits streamed: {units}; units at finish: {tail}; frames submitted: {FRAMES}");
    assert!(
        tail >= units && tail > 0,
        "the encoder's finish must account for every unit it streamed"
    );
    println!("load(1m) at end: {:.2}", load_1m());
}
