//! R42 (docs/soak-protocol.md): two threads polling one submission can panic wgpu-core 30.0.1's
//! `Device::maintain`, and the panic kills the encode thread that polled. `DrawDone::wait` does exactly
//! that on the record and stream threads, every 500 µs.
//!
//! The guard is the race at a rate worth reading: two threads poll each submission with
//! `Wait { Some(i), ZERO }`, as `DrawDone` does but with no sleep, while a third drains with
//! `PollType::Poll`. It is red on stock 30.0.1 and green with `vendor/wgpu-core`, so it is the test to
//! keep when that patch is dropped at wgpu 31. It counts frames and reads no clock.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Stock 30.0.1 panicked by frame 122 in 200 of 200 runs on the normative machine, median 10.
const FRAMES: u64 = 5_000;
const POLLERS: u64 = 2;

fn message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "a panic with no message".into())
}

#[test]
fn two_pollers_and_a_drain_loop_never_panic_the_device() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match pollster::block_on(instance.request_adapter(&Default::default())) {
        Ok(adapter) => adapter,
        Err(e) => {
            eprintln!("SKIP: no GPU adapter ({e})");
            return;
        }
    };
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).expect("a device");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());

    let current: Arc<Mutex<Option<(u64, wgpu::SubmissionIndex)>>> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let panicked: Arc<Mutex<Option<String>>> = Arc::default();
    let done = Arc::new(AtomicU64::new(0));

    let mut threads = Vec::new();
    for _ in 0..POLLERS {
        let (device, current, stop, panicked, done) = (
            device.clone(),
            current.clone(),
            stop.clone(),
            panicked.clone(),
            done.clone(),
        );
        threads.push(std::thread::spawn(move || {
            let mut seen = 0;
            while !stop.load(SeqCst) {
                let job = current.lock().unwrap().clone();
                let Some((n, index)) = job else { continue };
                if n == seen {
                    continue;
                }
                seen = n;
                while !stop.load(SeqCst) {
                    let poll = catch_unwind(AssertUnwindSafe(|| {
                        device.poll(wgpu::PollType::Wait {
                            submission_index: Some(index.clone()),
                            timeout: Some(Duration::ZERO),
                        })
                    }));
                    match poll {
                        Ok(Err(wgpu::PollError::Timeout)) => continue,
                        Ok(_) => {
                            done.fetch_add(1, SeqCst);
                            break;
                        }
                        Err(payload) => {
                            panicked.lock().unwrap().get_or_insert(message(payload));
                            stop.store(true, SeqCst);
                        }
                    }
                }
            }
        }));
    }
    {
        let (device, stop, panicked) = (device.clone(), stop.clone(), panicked.clone());
        threads.push(std::thread::spawn(move || {
            while !stop.load(SeqCst) {
                if let Err(payload) =
                    catch_unwind(AssertUnwindSafe(|| device.poll(wgpu::PollType::Poll)))
                {
                    panicked.lock().unwrap().get_or_insert(message(payload));
                    stop.store(true, SeqCst);
                }
            }
        }));
    }

    let mut frame = 0;
    while frame < FRAMES && !stop.load(SeqCst) {
        let mut encoder = device.create_command_encoder(&Default::default());
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLUE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        }));
        let index = queue.submit([encoder.finish()]);
        frame += 1;
        *current.lock().unwrap() = Some((frame, index));
        // Both pollers finish this frame before the next, so the queue goes empty between frames.
        while done.load(SeqCst) < frame * POLLERS && !stop.load(SeqCst) {
            std::hint::spin_loop();
        }
    }
    stop.store(true, SeqCst);
    for thread in threads {
        thread.join().unwrap();
    }

    let panicked = panicked.lock().unwrap().take();
    assert_eq!(
        panicked, None,
        "a poll panicked the device at frame {frame} of {FRAMES}"
    );
    assert_eq!(frame, FRAMES);
}
