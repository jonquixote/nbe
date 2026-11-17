//! Review4 step 3 (the user's ruling of 2026-11-16 23:45, work order 1): the
//! encoder releases what it has delivered, and a take flushes it at stop.
//!
//! Two faults, both verified by reading main 98d4bab:
//! - The VideoToolbox session kept every compressed unit for the whole take
//!   or stream (`units` append-only; `take_new_units` cloned and kept them).
//!   On real content that is about 2.3 GB an hour per output.
//! - A take's stop never completed the frames the encoder still held: the
//!   record thread finished the writer and dropped the encoder, whose `Drop`
//!   invalidates without `complete_frames`. The frames handed off last were
//!   not in the file.
//!
//! These need the hardware H.264 encoder (and AAC for the take); without it
//! they skip, as the other encode tests do.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nbe_engine::encode::EncodeSession;
use nbe_engine::record::{AudioTap, RecordMsg, RecordSession};

const W: u32 = 640;
const H: u32 = 360;
const FPS: u32 = 30;

fn encoder_or_skip() -> bool {
    if nbe_engine::encode::is_available() {
        true
    } else {
        eprintln!("SKIP: no hardware H.264 encoder on this machine");
        false
    }
}

/// A frame whose content changes every frame, so every frame encodes to a
/// real unit.
fn frame_rgba(i: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for (p, px) in rgba.chunks_exact_mut(4).enumerate() {
        let x = (p as u32) % W;
        let y = (p as u32) / W;
        px[0] = ((x + i * 7) % 256) as u8;
        px[1] = ((y + i * 3) % 256) as u8;
        px[2] = ((x ^ y ^ i) % 256) as u8;
        px[3] = 255;
    }
    rgba
}

#[test]
fn a_draining_session_holds_no_unit_it_has_delivered() {
    if !encoder_or_skip() {
        return;
    }
    let mut enc = EncodeSession::open(W, H, FPS, 4_000_000).expect("the encoder opens");
    enc.set_drain_delivered(true);
    let mut delivered = 0usize;
    for i in 0..60 {
        delivered += enc.encode_rgba(&frame_rgba(i)).expect("encode").len();
    }
    let held = enc.retained_delivered_units();
    println!("drain: 60 frames fed, {delivered} units delivered, {held} delivered units still held");
    assert!(
        delivered >= 50,
        "precondition: the encoder delivered units while fed ({delivered} of 60)"
    );
    assert_eq!(
        held, 0,
        "a draining session must not keep units it has delivered ({held} held of {delivered})"
    );
}

#[test]
fn flush_returns_every_frame_not_yet_delivered_and_nothing_twice() {
    if !encoder_or_skip() {
        return;
    }
    let n = 45u32;
    let mut enc = EncodeSession::open(W, H, FPS, 4_000_000).expect("the encoder opens");
    enc.set_drain_delivered(true);
    let mut pts = Vec::new();
    for i in 0..n {
        for u in enc.encode_rgba(&frame_rgba(i)).expect("encode") {
            pts.push(u.pts_seconds);
        }
    }
    let delivered = pts.len();
    let tail = enc.flush().expect("flush completes the session");
    let tail_len = tail.len();
    pts.extend(tail.iter().map(|u| u.pts_seconds));
    println!(
        "flush: {n} frames fed, {delivered} delivered while fed, {tail_len} in the flush's tail"
    );
    assert_eq!(
        pts.len(),
        n as usize,
        "every frame fed comes out once: {delivered} delivered + {tail_len} flushed"
    );
    let mut sorted = pts.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted.dedup();
    assert_eq!(sorted.len(), n as usize, "no unit is delivered twice");
    assert!(
        enc.encode_rgba(&frame_rgba(n)).is_err(),
        "a flushed session refuses further frames"
    );
}

/// Video samples (track 1) in a fragmented MP4: the sum of every video
/// `trun`'s sample count.
fn video_samples(path: &Path) -> usize {
    let bytes = std::fs::read(path).expect("the recording exists");
    let mut total = 0usize;
    walk(&bytes, &mut total, false);
    total
}

fn walk(buf: &[u8], total: &mut usize, mut in_video: bool) {
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let len = u32::from_be_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
        let kind = &buf[off + 4..off + 8];
        if len < 8 || off + len > buf.len() {
            break;
        }
        let body = &buf[off..off + len];
        match kind {
            b"moof" => walk(&body[8..], total, false),
            b"traf" => walk(&body[8..], total, false),
            b"tfhd" => {
                // version+flags at [8..12], track_ID at [12..16].
                in_video = u32::from_be_bytes(body[12..16].try_into().unwrap()) == 1;
            }
            b"trun" if in_video => {
                *total += u32::from_be_bytes(body[12..16].try_into().unwrap()) as usize;
            }
            _ => {}
        }
        off += len;
    }
}

#[test]
fn a_take_of_n_handed_off_frames_has_n_in_the_file() {
    if !encoder_or_skip() {
        return;
    }
    if !nbe_engine::record::aac::is_available() {
        eprintln!("SKIP: AudioToolbox AAC absent");
        return;
    }
    let n = 45u32;
    let dir = tempfile::tempdir().expect("tempdir");
    let tap = Arc::new(AudioTap::new());
    // Two seconds of a quiet tone, so the take has audio throughout.
    let pcm: Vec<f32> = (0..96_000)
        .flat_map(|k| {
            let v = 0.1 * (2.0 * std::f32::consts::PI * 440.0 * k as f32 / 48_000.0).sin();
            [v, v]
        })
        .collect();
    tap.push(&pcm);
    let skipped = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut session = RecordSession::open(
        dir.path(),
        "drainshow",
        "ep01",
        "20261117T000000Z",
        W,
        H,
        FPS,
        tap,
        skipped.clone(),
    )
    .expect("the take opens");
    let tx = session.frame_sender();
    for i in 0..n {
        // A blocking send: no frame is shed on the way in.
        tx.send(RecordMsg::Frame {
            rgba: frame_rgba(i),
            frame: Some(i as u64),
        })
        .expect("the record thread takes the frame");
    }
    drop(tx);
    let path = session
        .stop_and_finish(Duration::from_secs(20))
        .expect("the take finishes");
    let in_file = video_samples(&path);
    let shed = skipped.load(std::sync::atomic::Ordering::SeqCst);
    println!("take: {n} frames handed off, {shed} shed, {in_file} video samples in the file");
    assert_eq!(shed, 0, "precondition: no frame was shed");
    assert_eq!(
        in_file, n as usize,
        "every frame handed off before the stop is in the file"
    );
}
