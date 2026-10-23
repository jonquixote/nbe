//! Throwaway device-audio probe for the device-audio design record (D1, D2).
//! Not part of the repo. Subcommands:
//!   info
//!   tone <file> <gain_db> <device> [mic]
//!   underrun <device> <stall_ms> <at_callback> <loopback:0|1>
//!   drift <secs> <device> <csv_out>
//! <device> is "default" or a substring of a CoreAudio device name.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use objc2_core_audio::*;
use objc2_core_foundation::{CFRetained, CFString};
use std::ffi::c_void;
use std::ptr::{null, NonNull};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn addr(sel: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: sel, mScope: scope, mElement: kAudioObjectPropertyElementMain }
}

fn get<T: Default + Copy>(id: u32, sel: u32, scope: u32) -> Option<T> {
    let a = addr(sel, scope);
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    let st = unsafe {
        AudioObjectGetPropertyData(id, NonNull::from(&a), 0, null(), NonNull::from(&mut size), NonNull::from(&mut v).cast())
    };
    (st == 0).then_some(v)
}

fn get_vec_u32(id: u32, sel: u32, scope: u32) -> Vec<u32> {
    let a = addr(sel, scope);
    let mut size = 0u32;
    let st = unsafe { AudioObjectGetPropertyDataSize(id, NonNull::from(&a), 0, null(), NonNull::from(&mut size)) };
    if st != 0 || size == 0 {
        return vec![];
    }
    let mut v = vec![0u32; size as usize / 4];
    let st = unsafe {
        AudioObjectGetPropertyData(id, NonNull::from(&a), 0, null(), NonNull::from(&mut size), NonNull::new(v.as_mut_ptr()).unwrap().cast())
    };
    if st != 0 { vec![] } else { v }
}

fn name(id: u32) -> String {
    let a = addr(kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal);
    let mut p: *const CFString = null();
    let mut size = std::mem::size_of::<*const CFString>() as u32;
    let st = unsafe {
        AudioObjectGetPropertyData(id, NonNull::from(&a), 0, null(), NonNull::from(&mut size), NonNull::from(&mut p).cast())
    };
    if st != 0 || p.is_null() {
        return format!("<id {id}>");
    }
    let s: CFRetained<CFString> = unsafe { CFRetained::from_raw(NonNull::new(p as *mut CFString).unwrap()) };
    s.to_string()
}

fn sys() -> u32 {
    kAudioObjectSystemObject as u32
}

fn resolve(dev: &str) -> u32 {
    if dev == "default" {
        return get::<u32>(sys(), kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal).unwrap();
    }
    get_vec_u32(sys(), kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal)
        .into_iter()
        .find(|&id| name(id).contains(dev))
        .unwrap_or_else(|| panic!("no device named like {dev}"))
}

fn cpal_out(host: &cpal::Host, dev: &str) -> cpal::Device {
    if dev == "default" {
        return host.default_output_device().unwrap();
    }
    host.output_devices().unwrap().find(|d| d.description().map(|x| x.name().contains(dev)).unwrap_or(false)).unwrap()
}

fn cpal_in(host: &cpal::Host, dev: &str) -> cpal::Device {
    if dev == "default" {
        return host.default_input_device().unwrap();
    }
    host.input_devices().unwrap().find(|d| d.description().map(|x| x.name().contains(dev)).unwrap_or(false)).unwrap()
}

fn nominal(id: u32) -> f64 {
    get::<f64>(id, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal).unwrap_or(f64::NAN)
}

fn set_nominal(id: u32, rate: f64) -> i32 {
    let a = addr(kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal);
    unsafe { AudioObjectSetPropertyData(id, NonNull::from(&a), 0, null(), 8, NonNull::from(&rate).cast()) }
}

fn describe(id: u32) {
    let out = kAudioObjectPropertyScopeOutput;
    let streams = get_vec_u32(id, kAudioDevicePropertyStreams, out);
    let stream_lat: Vec<Option<u32>> = streams.iter().map(|&s| get::<u32>(s, kAudioStreamPropertyLatency, kAudioObjectPropertyScopeGlobal)).collect();
    println!(
        "device {id} \"{}\": nominal {} Hz, actual {:?} Hz, buffer {:?} frames, device latency (out) {:?} frames, safety offset (out) {:?} frames, output streams {:?}, stream latency {:?} frames",
        name(id),
        nominal(id),
        get::<f64>(id, kAudioDevicePropertyActualSampleRate, kAudioObjectPropertyScopeGlobal),
        get::<u32>(id, kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal),
        get::<u32>(id, kAudioDevicePropertyLatency, out),
        get::<u32>(id, kAudioDevicePropertySafetyOffset, out),
        streams,
        stream_lat,
    );
}

fn ns(i: Instant, base: Instant) -> u64 {
    i.duration_since(base).as_nanos() as u64
}

fn info() {
    let host = cpal::default_host();
    println!("cpal host: {:?}", host.id());
    for dev in ["default", "VB-Cable"] {
        let id = resolve(dev);
        describe(id);
        let d = cpal_out(&host, dev);
        println!("  cpal default output config: {:?}", d.default_output_config());
        if let Ok(cfgs) = d.supported_output_configs() {
            for c in cfgs {
                println!("  cpal supported: {:?}", c);
            }
        }
    }
}

/// A file in, a tone out (D1).
fn tone(file: &str, gain_db: f32, dev: &str, mic: bool) {
    let track = nbe_decode::decode_audio(std::path::Path::new(file)).expect("decode").expect("an audio track");
    let gain = 10f32.powf(gain_db / 20.0);
    let peak = track.samples.iter().fold(0f32, |m, &x| m.max(x.abs()));
    println!(
        "decoded {file}: {} Hz, {} ch, {} frames ({:.3} s), peak {:.1} dBFS; playing at {gain_db} dB",
        track.sample_rate, track.channels, track.frames(), track.frames() as f64 / track.sample_rate as f64, 20.0 * peak.log10()
    );
    let id = resolve(dev);
    let before = nominal(id);
    describe(id);
    let host = cpal::default_host();
    let device = cpal_out(&host, dev);
    let config = cpal::StreamConfig { channels: 2, sample_rate: 48_000, buffer_size: cpal::BufferSize::Default };
    let samples = Arc::new(track.samples);
    let pos = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = sync_channel::<(u64, u64, u64, usize, usize)>(1 << 16);
    let t0 = Instant::now();
    let first: Arc<AtomicU64> = Arc::new(AtomicU64::new(u64::MAX));
    let (s2, p2, f2) = (samples.clone(), pos.clone(), first.clone());
    let mut base: Option<cpal::StreamInstant> = None;
    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let ts = info.timestamp();
                let b = *base.get_or_insert(ts.callback);
                let p = p2.load(Ordering::Relaxed);
                let mut first_nonzero = usize::MAX;
                for (i, o) in out.iter_mut().enumerate() {
                    let v = s2.get(p + i).copied().unwrap_or(0.0) * gain;
                    if v.abs() > 1e-4 && first_nonzero == usize::MAX {
                        first_nonzero = i / 2;
                    }
                    *o = v;
                }
                p2.store(p + out.len(), Ordering::Relaxed);
                let cb = ts.callback.duration_since(b).as_nanos() as u64;
                let pb = ts.playback.duration_since(b).as_nanos() as u64;
                if first_nonzero != usize::MAX && f2.load(Ordering::Relaxed) == u64::MAX {
                    // playback instant of the first audible frame, in ns since the first callback
                    f2.store(pb + first_nonzero as u64 * 1_000_000_000 / 48_000, Ordering::Relaxed);
                }
                let _ = tx.try_send((ns(Instant::now(), t0), cb, pb, out.len() / 2, first_nonzero));
            },
            |e| eprintln!("cpal error callback: {e}"),
            None,
        )
        .expect("build output stream");
    let during = nominal(id);
    // Optional microphone capture (acoustic round trip).
    let cap: Arc<std::sync::Mutex<Vec<(u64, f32)>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut in_stream = None;
    let mut in_rate = 0;
    if mic {
        let idev = cpal_in(&host, "default");
        let icfg = idev.default_input_config().unwrap();
        in_rate = icfg.sample_rate();
        let ich = icfg.channels() as usize;
        println!("mic: {:?} {:?}", idev.description().map(|d| d.name().to_string()), icfg);
        let c2 = cap.clone();
        let s = idev
            .build_input_stream(
                icfg.config(),
                move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                    let now = ns(Instant::now(), t0);
                    let mut g = c2.lock().unwrap();
                    let frames = data.len() / ich;
                    for f in 0..frames {
                        // host time of this frame, back-dated from the callback's arrival
                        let back = (frames - f) as u64 * 1_000_000_000 / in_rate as u64;
                        g.push((now.saturating_sub(back), data[f * ich]));
                    }
                },
                |e| eprintln!("mic error: {e}"),
                None,
            )
            .expect("input stream");
        s.play().unwrap();
        in_stream = Some(s);
        std::thread::sleep(Duration::from_millis(300));
    }
    let start_ns = ns(Instant::now(), t0);
    stream.play().unwrap();
    let total = samples.len();
    while pos.load(Ordering::Relaxed) < total {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(400));
    drop(stream);
    drop(in_stream);
    let after = nominal(id);
    let rows: Vec<_> = rx.try_iter().collect();
    let frames: Vec<usize> = rows.iter().map(|r| r.3).collect();
    let lat: Vec<f64> = rows.iter().map(|r| (r.2 - r.1) as f64 / 1e6).collect();
    println!(
        "callbacks {}: frames per callback {:?}..{:?}, playback-callback {:.2}..{:.2} ms, first callback {:.1} ms after play()",
        rows.len(),
        frames.iter().min(),
        frames.iter().max(),
        lat.iter().cloned().fold(f64::INFINITY, f64::min),
        lat.iter().cloned().fold(0.0, f64::max),
        (rows.first().map(|r| r.0).unwrap_or(0) as f64 - start_ns as f64) / 1e6
    );
    println!("nominal rate: before {before}, while open {during}, after close {after}");
    if (after - before).abs() > 0.5 {
        let st = set_nominal(id, before);
        std::thread::sleep(Duration::from_millis(300));
        println!("restored nominal rate to {before}: status {st}, now {}", nominal(id));
    }
    if mic {
        let cap = cap.lock().unwrap();
        // The tone's own frequency, from the decoded file's zero crossings.
        let ch0: Vec<f32> = samples.iter().step_by(2).cloned().collect();
        let zc = ch0.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let hz = zc as f64 / (ch0.len() as f64 / 48_000.0);
        // Goertzel power at that frequency over 10 ms windows of the capture.
        let win = in_rate as usize / 100;
        let k = 2.0 * (2.0 * std::f64::consts::PI * hz / in_rate as f64).cos();
        let powers: Vec<(u64, f64)> = cap
            .chunks(win)
            .filter(|c| c.len() == win)
            .map(|c| {
                let (mut s1, mut s2) = (0f64, 0f64);
                for x in c {
                    let s = x.1 as f64 + k * s1 - s2;
                    s2 = s1;
                    s1 = s;
                }
                (c[0].0, s1 * s1 + s2 * s2 - k * s1 * s2)
            })
            .collect();
        let mut pre: Vec<f64> = powers.iter().take(20).map(|p| p.1).collect();
        pre.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = pre.get(pre.len() / 2).cloned().unwrap_or(0.0).max(1e-12);
        let hit = powers.iter().find(|p| p.1 > 100.0 * med);
        let peak = powers.iter().cloned().fold(0f64, |m, p| m.max(p.1));
        println!(
            "mic Goertzel at the tone's {hz:.1} Hz: pre-roll median power {med:.3e}, peak {peak:.3e} ({:.1} dB over), first 10 ms window above +20 dB at {:?} ms",
            10.0 * (peak / med).log10(),
            hit.map(|h| h.0 as f64 / 1e6)
        );
        let mut w = String::from("t_ns,x\n");
        for x in cap.iter() {
            w.push_str(&format!("{},{}\n", x.0, x.1));
        }
        let _ = std::fs::write("mic-capture.csv", w);
        if let (Some(h), Some(r)) = (hit, rows.iter().find(|r| r.4 != usize::MAX)) {
            let out_audible = r.0 as f64 + (r.2 - r.1) as f64 + r.4 as f64 * 1e9 / 48_000.0;
            println!("Goertzel onset minus cpal's predicted first audible frame: {:.1} ms (10 ms window resolution)", (h.0 as f64 - out_audible) / 1e6);
        }
        let floor = cap.iter().take(in_rate as usize / 10).fold(0f32, |m, x| m.max(x.1.abs()));
        let onset = cap.iter().find(|x| x.1.abs() > (floor * 4.0).max(0.01));
        println!("mic: {} samples captured, pre-roll floor {:.4}, onset {:?}", cap.len(), floor, onset);
        // The first audible output frame in Instant terms: the callback's arrival is the anchor.
        if let (Some(o), Some(r)) = (onset, rows.iter().find(|r| r.4 != usize::MAX)) {
            let out_audible = r.0 as f64 + (r.2 - r.1) as f64 + r.4 as f64 * 1e9 / 48_000.0;
            println!(
                "acoustic: first audible frame predicted at {:.1} ms (callback arrival + cpal playback delay), mic onset at {:.1} ms (arrival back-dated): round trip beyond the prediction {:.1} ms",
                out_audible / 1e6,
                o.0 as f64 / 1e6,
                (o.0 as f64 - out_audible) / 1e6
            );
        }
    }
}

/// A callback deliberately late once (D1: underrun behaviour named).
fn underrun(dev: &str, stall_ms: u64, at: usize, loopback: bool) {
    let host = cpal::default_host();
    let id = resolve(dev);
    describe(id);
    let device = cpal_out(&host, dev);
    let config = cpal::StreamConfig { channels: 2, sample_rate: 48_000, buffer_size: cpal::BufferSize::Default };
    let t0 = Instant::now();
    let (tx, rx) = sync_channel::<(usize, u64, u64, u64, usize)>(1 << 16);
    let errors = Arc::new(AtomicUsize::new(0));
    let e2 = errors.clone();
    let mut n = 0usize;
    let mut phase_frames: u64 = 0;
    let mut base: Option<cpal::StreamInstant> = None;
    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let ts = info.timestamp();
                let b = *base.get_or_insert(ts.callback);
                let arrive = ns(Instant::now(), t0);
                if n == at {
                    std::thread::sleep(Duration::from_millis(stall_ms));
                }
                let frames = out.len() / 2;
                for f in 0..frames {
                    let k = phase_frames + f as u64;
                    let v = 0.1 * (2.0 * std::f64::consts::PI * 1000.0 * k as f64 / 48_000.0).sin() as f32;
                    out[2 * f] = v;
                    out[2 * f + 1] = v;
                }
                phase_frames += frames as u64;
                let _ = tx.try_send((n, arrive, ts.callback.duration_since(b).as_nanos() as u64, ts.playback.duration_since(b).as_nanos() as u64, frames));
                n += 1;
            },
            move |e| {
                e2.fetch_add(1, Ordering::Relaxed);
                eprintln!("cpal error callback: {e}");
            },
            None,
        )
        .expect("build output stream");
    let cap: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::with_capacity(48_000 * 6)));
    let mut in_stream = None;
    if loopback {
        let idev = cpal_in(&host, dev);
        let icfg = cpal::StreamConfig { channels: 2, sample_rate: 48_000, buffer_size: cpal::BufferSize::Default };
        let c2 = cap.clone();
        let s = idev
            .build_input_stream(icfg, move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mut g = c2.lock().unwrap();
                for f in 0..data.len() / 2 {
                    g.push(data[2 * f]);
                }
            }, |e| eprintln!("loopback input error: {e}"), None)
            .expect("loopback input");
        s.play().unwrap();
        in_stream = Some(s);
    }
    stream.play().unwrap();
    std::thread::sleep(Duration::from_secs(3));
    drop(stream);
    std::thread::sleep(Duration::from_millis(200));
    drop(in_stream);
    let rows: Vec<_> = rx.try_iter().collect();
    let buf = rows.get(1).map(|r| r.4).unwrap_or(0);
    let period_ms = buf as f64 / 48.0;
    println!("callbacks {}, buffer {} frames ({:.2} ms), stall {} ms at callback {}, cpal error callbacks {}", rows.len(), buf, period_ms, stall_ms, at, errors.load(Ordering::Relaxed));
    for r in rows.iter().skip(at.saturating_sub(2)).take(7) {
        println!("  cb {:>4}: arrived {:>9.3} ms, HAL callback time {:>9.3} ms, playback {:>9.3} ms, frames {}", r.0, r.1 as f64 / 1e6, r.2 as f64 / 1e6, r.3 as f64 / 1e6, r.4);
    }
    // HAL cycles skipped: the callback-time step across the stall, in buffer periods.
    if let (Some(a), Some(b)) = (rows.get(at), rows.get(at + 1)) {
        println!("HAL callback-time step across the stall: {:.3} ms = {:.2} buffer periods", (b.2 as f64 - a.2 as f64) / 1e6, (b.2 as f64 - a.2 as f64) / 1e6 / period_ms);
    }
    if loopback {
        let cap = cap.lock().unwrap();
        // Find runs of silence and phase jumps in the captured sine.
        let start = cap.iter().position(|x| x.abs() > 0.01).unwrap_or(0);
        let mut runs = vec![];
        let mut i = start;
        while i < cap.len() {
            if cap[i].abs() < 1e-6 {
                let j = (i..cap.len()).find(|&j| cap[j].abs() >= 1e-6).unwrap_or(cap.len());
                if j - i >= 8 && j < cap.len() {
                    runs.push((i - start, j - i));
                }
                i = j;
            } else {
                i += 1;
            }
        }
        println!("loopback: {} frames captured from first signal; silent runs (offset, frames) {:?}", cap.len() - start, runs);
    }
}

/// Silent output for `secs`: the device's sample clock against the host clock (D2).
fn drift(secs: u64, dev: &str, csv: &str) {
    let host = cpal::default_host();
    let id = resolve(dev);
    describe(id);
    let device = cpal_out(&host, dev);
    let nom = nominal(id);
    let config = cpal::StreamConfig { channels: 2, sample_rate: nom as u32, buffer_size: cpal::BufferSize::Default };
    println!("drift: {secs} s of silence on \"{}\" at its own nominal {nom} Hz (no rate change)", name(id));
    let t0 = Instant::now();
    let (tx, rx) = sync_channel::<(u64, u64, usize)>(1 << 20);
    let mut base: Option<cpal::StreamInstant> = None;
    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let ts = info.timestamp();
                let b = *base.get_or_insert(ts.callback);
                out.fill(0.0);
                let _ = tx.try_send((ns(Instant::now(), t0), ts.callback.duration_since(b).as_nanos() as u64, out.len() / 2));
            },
            |e| eprintln!("cpal error callback: {e}"),
            None,
        )
        .expect("build output stream");
    stream.play().unwrap();
    let mut actual = vec![];
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_secs(1));
        if let Some(a) = get::<f64>(id, kAudioDevicePropertyActualSampleRate, kAudioObjectPropertyScopeGlobal) {
            actual.push(a);
        }
    }
    drop(stream);
    let rows: Vec<_> = rx.try_iter().collect();
    let mut w = String::from("arrive_ns,hal_ns,frames\n");
    for r in &rows {
        w.push_str(&format!("{},{},{}\n", r.0, r.1, r.2));
    }
    std::fs::write(csv, w).unwrap();
    // Least squares: cumulative frames before each callback against its HAL time, and against arrival.
    let fit = |xs: &[f64], ys: &[f64]| {
        let n = xs.len() as f64;
        let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
        let sxy: f64 = xs.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)).sum();
        let sxx: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
        let slope = sxy / sxx;
        let resid: Vec<f64> = xs.iter().zip(ys).map(|(x, y)| y - (my + slope * (x - mx))).collect();
        let rms = (resid.iter().map(|r| r * r).sum::<f64>() / n).sqrt();
        (slope, rms)
    };
    let mut cum = 0f64;
    let mut ys = vec![];
    for r in &rows {
        ys.push(cum);
        cum += r.2 as f64;
    }
    let hal: Vec<f64> = rows.iter().map(|r| r.1 as f64 / 1e9).collect();
    let arr: Vec<f64> = rows.iter().map(|r| r.0 as f64 / 1e9).collect();
    let (rh, eh) = fit(&hal, &ys);
    let (ra, ea) = fit(&arr, &ys);
    let ppm_h = (rh / nom - 1.0) * 1e6;
    let ppm_a = (ra / nom - 1.0) * 1e6;
    let mean_actual = actual.iter().sum::<f64>() / actual.len().max(1) as f64;
    println!("callbacks {}, frames {}, span {:.1} s (HAL time)", rows.len(), cum, hal.last().unwrap_or(&0.0));
    println!("rate vs HAL host time: {:.4} Hz ({:+.2} ppm vs nominal), fit residual rms {:.1} frames", rh, ppm_h, eh);
    println!("rate vs callback arrival (Instant): {:.4} Hz ({:+.2} ppm), residual rms {:.1} frames", ra, ppm_a, ea);
    println!(
        "kAudioDevicePropertyActualSampleRate: mean {:.4} Hz ({:+.2} ppm), min {:.4}, max {:.4}, {} polls",
        mean_actual,
        (mean_actual / nom - 1.0) * 1e6,
        actual.iter().cloned().fold(f64::INFINITY, f64::min),
        actual.iter().cloned().fold(0.0, f64::max),
        actual.len()
    );
    for m in [1.0, 10.0, 30.0] {
        println!("  skew at {m:>4} min if uncorrected: {:+.2} ms (HAL fit)", ppm_h * 1e-6 * m * 60.0 * 1000.0);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("info") => info(),
        Some("tone") => tone(&a[2], a[3].parse().unwrap(), &a[4], a.get(5).map(|s| s == "mic").unwrap_or(false)),
        Some("underrun") => underrun(&a[2], a[3].parse().unwrap(), a[4].parse().unwrap(), a[5] == "1"),
        Some("drift") => drift(a[2].parse().unwrap(), &a[3], &a[4]),
        Some("setrate") => {
            let id = resolve(&a[2]);
            let before = nominal(id);
            let st = set_nominal(id, a[3].parse().unwrap());
            std::thread::sleep(Duration::from_millis(300));
            println!("\"{}\": nominal {before} -> {} (status {st})", name(id), nominal(id));
        }
        _ => eprintln!("usage: info | tone <file> <gain_db> <device> [mic] | underrun <device> <stall_ms> <at> <loopback 0|1> | drift <secs> <device> <csv>"),
    }
}
