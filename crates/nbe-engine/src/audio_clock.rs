//! The device's clock against the master clock (device-audio design D2).
//!
//! The View holds master. The device runs its own crystal (+12.76 ppm on the
//! normative machine's speakers, design record §2.4), so contiguous playback
//! drifts against the picture: +23 ms after 30 minutes. This module keeps the
//! two together without touching the real-time thread:
//!
//! - the device callback writes one **presentation mark** per callback, the
//!   ring frame it starts on and the instant that frame is heard
//!   ([`DeviceMarks`]);
//! - the audio thread's [`Feeder`] plans each block against the latest mark:
//!   which master sample the block starts at, and whether it is re-spaced one
//!   sample short or long (a **slip**) to hold the skew inside the threshold;
//! - every pushed block is logged ([`PushLog`]), so the skew of what the
//!   device actually presented can be read back: `audioDriftMs` as SPEC §8.9
//!   defines it, device-consumed against master-implied (R27).
//!
//! Everything here is plain arithmetic over atomics and fixed arrays, so it
//! runs, and is tested, without a device.

use std::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::audio::{CHANNELS, SAMPLE_RATE};

/// Past this skew, a block is re-spaced one sample short or long (D2's
/// threshold; proposed in the build PR, ruled there).
pub const SLIP_THRESHOLD_MS: f64 = 0.5;

/// Past this skew, slipping one sample a block would take seconds, so the
/// cursor re-anchors at once (proposed in the build PR, ruled there). It
/// happens only after a gap the device itself made audible: an underrun, or
/// the clock starting.
pub const REALIGN_MS: f64 = 4.0;

const NS_PER_SAMPLE: f64 = 1e9 / SAMPLE_RATE as f64;

/// One presentation mark: ring frame `frame` is heard `ns` after the marks'
/// base instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    pub frame: u64,
    pub ns: u64,
}

/// What the device callback publishes. Written by the callback alone, read by
/// the audio thread; every write is an atomic store, so the callback never
/// waits on the reader.
#[derive(Debug)]
pub struct DeviceMarks {
    base: Instant,
    /// A sequence lock over `mark_frame` and `mark_ns`: odd while a write is
    /// in progress, so a reader never pairs one callback's frame with another's
    /// instant.
    seq: AtomicU64,
    mark_frame: AtomicU64,
    mark_ns: AtomicU64,
    /// Callbacks served.
    pub callbacks: AtomicU64,
    /// Callbacks the ring could not fill: each is an underrun (SPEC §8.10).
    pub short_callbacks: AtomicU64,
    /// Frames of silence those callbacks emitted.
    pub short_frames: AtomicU64,
    /// Overloads the device reported (cpal's `Xrun`): the HAL skipped cycles,
    /// a hole of silence on the device's own timeline.
    pub xruns: AtomicU64,
    /// The device went away (unplugged, invalidated).
    pub lost: AtomicBool,
    /// Test seam for the normative machine's forced-stall case: the next
    /// callback sleeps this many ms before copying out, which the HAL reports
    /// as an overload (design record §2.3's method). Never set in production;
    /// the callback's cost is one atomic swap.
    pub stall_next_ms: AtomicU64,
}

impl DeviceMarks {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            seq: AtomicU64::new(0),
            mark_frame: AtomicU64::new(0),
            mark_ns: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            short_callbacks: AtomicU64::new(0),
            short_frames: AtomicU64::new(0),
            xruns: AtomicU64::new(0),
            lost: AtomicBool::new(false),
            stall_next_ms: AtomicU64::new(0),
        })
    }

    /// The instant every mark is measured from.
    pub fn base(&self) -> Instant {
        self.base
    }

    /// `instant` as ns after the base, saturating at zero.
    pub fn ns_of(&self, instant: Instant) -> u64 {
        instant.saturating_duration_since(self.base).as_nanos() as u64
    }

    /// `instant` as signed ns relative to the base.
    pub fn signed_ns_of(&self, instant: Instant) -> i64 {
        match instant.checked_duration_since(self.base) {
            Some(d) => d.as_nanos() as i64,
            None => -(self.base.duration_since(instant).as_nanos() as i64),
        }
    }

    /// The callback's write: ring frame `frame` is heard at `ns`.
    pub fn mark(&self, frame: u64, ns: u64) {
        self.seq.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        self.mark_frame.store(frame, Ordering::Relaxed);
        self.mark_ns.store(ns, Ordering::Relaxed);
        self.seq.fetch_add(1, Ordering::Release);
        self.callbacks.fetch_add(1, Ordering::Relaxed);
    }

    /// The latest mark, or `None` before the first callback.
    pub fn latest(&self) -> Option<Mark> {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let frame = self.mark_frame.load(Ordering::Relaxed);
            let ns = self.mark_ns.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return (s1 > 0).then_some(Mark { frame, ns });
            }
        }
    }
}

/// The device callback's whole body (D2): mark when the buffer's first frame
/// is heard, copy the ring out, and silence and count what the ring could not
/// fill. Atomics and copies only: no lock, no allocation, nothing that waits
/// (SPEC §8.9). `presented_ns` is the instant the buffer's first frame is
/// heard, ns after the marks' base.
pub fn copy_out(
    consumer: &mut crate::audio_ring::RingConsumer,
    marks: &DeviceMarks,
    out: &mut [f32],
    presented_ns: u64,
) {
    marks.mark(consumer.read_frames(), presented_ns);
    let started = consumer.started();
    let n = consumer.pop_into(out);
    if n < out.len() {
        out[n..].fill(0.0);
        if !started {
            // The device opened before the audio thread's first block: the
            // stream has not started, and its silence is not an underrun
            // (found on the normative machine: 16 such callbacks counted
            // at an open, which would read as underruns at every boot).
            return;
        }
        marks.short_callbacks.fetch_add(1, Ordering::Relaxed);
        marks
            .short_frames
            .fetch_add(((out.len() - n) / CHANNELS) as u64, Ordering::Relaxed);
    }
}

/// One pushed block: ring frames `ring_start..ring_start + out_frames` carry
/// master samples `master_start..master_start + in_frames`, re-spaced end to
/// end.
#[derive(Debug, Clone, Copy, Default)]
struct PushRecord {
    ring_start: u64,
    master_start: i64,
    in_frames: u32,
    out_frames: u32,
    /// The clock's epoch (ns after the marks' base) the block was rendered
    /// against; `None` for a block rendered while the clock was STOPPED.
    epoch_ns: Option<i64>,
}

const LOG_LEN: usize = 32;

/// The last [`LOG_LEN`] pushes: more than the ring holds, so every frame the
/// device can be presenting is still in the log.
#[derive(Debug)]
pub struct PushLog {
    entries: [PushRecord; LOG_LEN],
    next: usize,
}

impl Default for PushLog {
    fn default() -> Self {
        Self {
            entries: [PushRecord::default(); LOG_LEN],
            next: 0,
        }
    }
}

impl PushLog {
    fn push(&mut self, r: PushRecord) {
        self.entries[self.next % LOG_LEN] = r;
        self.next += 1;
    }

    /// The master sample ring frame `frame` carries, and the epoch it was
    /// rendered against; `None` if the frame is not logged or was rendered
    /// while the clock was stopped.
    fn master_at(&self, frame: u64) -> Option<(f64, i64)> {
        self.entries.iter().find_map(|r| {
            let epoch = r.epoch_ns?;
            if r.out_frames == 0
                || frame < r.ring_start
                || frame >= r.ring_start + r.out_frames as u64
            {
                return None;
            }
            let k = (frame - r.ring_start) as f64;
            let step = if r.out_frames > 1 {
                (r.in_frames as f64 - 1.0) / (r.out_frames as f64 - 1.0)
            } else {
                1.0
            };
            Some((r.master_start as f64 + k * step, epoch))
        })
    }
}

/// What the next block is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    /// The master sample the block's first frame is rendered at.
    pub base_sample: i64,
    /// Output frames: the block's length, or one fewer (a drop) or one more
    /// (a duplicate) when it slips.
    pub out_frames: usize,
    /// The clock's epoch the block is rendered against, `None` while STOPPED.
    pub epoch_ns: Option<i64>,
    /// The skew this block will be heard at before any slip, in ms: positive
    /// is late against the master clock.
    pub skew_ms: f64,
}

/// The audio thread's side of the clock (D2's architecture: master-clock
/// render, lock-free buffer, device copy-out).
#[derive(Debug)]
pub struct Feeder {
    block_frames: usize,
    log: PushLog,
    /// The next master sample to render while the clock runs.
    cursor: Option<i64>,
    /// The epoch `cursor` is anchored to.
    anchored: Option<i64>,
    /// The previous block's last input frame: the cubic's left neighbour, so
    /// a re-spaced block joins its predecessor without a step.
    prev: [f32; CHANNELS],
    out: Vec<f32>,
    pub drops: u64,
    pub duplicates: u64,
    pub realigns: u64,
    /// Test seam: the slip disabled, or its sign reversed (M-slip-off,
    /// M-slip-sign).
    pub slip_gain: i32,
    /// Test seam for the normative loopback's slip-residual case: a ppm the
    /// planner adds to the device's clock. VB-Cable is clocked from the host
    /// (0 ppm, design record §2.4), so corrections through it must be forced.
    pub inject_ppm: f64,
}

impl Feeder {
    pub fn new(block_frames: usize) -> Self {
        Self {
            block_frames,
            log: PushLog::default(),
            cursor: None,
            anchored: None,
            prev: [0.0; CHANNELS],
            out: vec![0.0; (block_frames + 1) * CHANNELS],
            drops: 0,
            duplicates: 0,
            realigns: 0,
            slip_gain: 1,
            inject_ppm: 0.0,
        }
    }

    /// Plan the next block.
    ///
    /// - `epoch_ns`: the master clock's epoch, ns after the marks' base;
    ///   `None` while STOPPED.
    /// - `next_ring_frame`: the ring index the block's first frame takes.
    /// - `mark`: the device's latest presentation mark.
    /// - `now_ns`, `fallback_lead_frames`: before the first callback, the
    ///   block is predicted to be heard this many frames from now.
    pub fn plan(
        &mut self,
        epoch_ns: Option<i64>,
        next_ring_frame: u64,
        mark: Option<Mark>,
        now_ns: u64,
        fallback_lead_frames: u64,
    ) -> Plan {
        let block = self.block_frames;
        let Some(epoch) = epoch_ns else {
            // Stopped: the graph renders at master frame 0, as the null sink's
            // cadence does, and nothing is aligned to a clock that is not
            // running.
            self.cursor = None;
            self.anchored = None;
            return Plan {
                base_sample: 0,
                out_frames: block,
                epoch_ns: None,
                skew_ms: 0.0,
            };
        };
        // When the block's first frame will be heard, in ns after the base.
        let heard = match mark {
            Some(m) => m.ns as f64 + next_ring_frame.saturating_sub(m.frame) as f64 * NS_PER_SAMPLE,
            None => now_ns as f64 + fallback_lead_frames as f64 * NS_PER_SAMPLE,
        } * (1.0 + self.inject_ppm * 1e-6);
        let anchor = |heard: f64| ((heard - epoch as f64) / NS_PER_SAMPLE).round() as i64;
        if self.anchored != Some(epoch) || self.cursor.is_none() {
            // The clock started: the cursor starts at the master sample due
            // when this block is heard, so the latency is compensated from the
            // first frame.
            self.cursor = Some(anchor(heard));
            self.anchored = Some(epoch);
        }
        let mut cursor = self.cursor.expect("anchored above");
        let mut skew_ms = (heard - (epoch as f64 + cursor as f64 * NS_PER_SAMPLE)) / 1e6;
        if skew_ms.abs() > REALIGN_MS {
            cursor = anchor(heard);
            self.realigns += 1;
            skew_ms = (heard - (epoch as f64 + cursor as f64 * NS_PER_SAMPLE)) / 1e6;
        }
        // Late (positive skew): drop a sample, so master time runs ahead of
        // the device's frames. Early: duplicate one.
        let slip = if skew_ms > SLIP_THRESHOLD_MS {
            -1
        } else if skew_ms < -SLIP_THRESHOLD_MS {
            1
        } else {
            0
        } * self.slip_gain;
        match slip.signum() {
            -1 => self.drops += 1,
            1 => self.duplicates += 1,
            _ => {}
        }
        self.cursor = Some(cursor + block as i64);
        Plan {
            base_sample: cursor,
            out_frames: (block as i64 + slip.signum() as i64) as usize,
            epoch_ns: Some(epoch),
            skew_ms,
        }
    }

    /// Re-space the rendered block to the plan's length, log it, and return
    /// the frames to push. A block that does not slip passes through bit for
    /// bit.
    pub fn emit(&mut self, rendered: &[f32], plan: &Plan, ring_start: u64) -> &[f32] {
        let n = plan.out_frames;
        let out = &mut self.out[..n * CHANNELS];
        respace(rendered, self.prev, out);
        let frames = rendered.len() / CHANNELS;
        if frames > 0 {
            self.prev
                .copy_from_slice(&rendered[(frames - 1) * CHANNELS..frames * CHANNELS]);
        }
        self.log.push(PushRecord {
            ring_start,
            master_start: plan.base_sample,
            in_frames: frames as u32,
            out_frames: n as u32,
            epoch_ns: plan.epoch_ns,
        });
        &self.out[..n * CHANNELS]
    }

    /// The skew the device actually presented at `mark`, in ms (SPEC §8.9's
    /// drift: device-consumed against master-implied). Positive is late.
    /// `None` while nothing rendered against the running clock is playing.
    pub fn presented_skew_ms(&self, mark: Mark) -> Option<f64> {
        let (master, epoch) = self.log.master_at(mark.frame)?;
        Some((mark.ns as f64 - (epoch as f64 + master * NS_PER_SAMPLE)) / 1e6)
    }
}

/// Re-space `input` (interleaved frames) onto `out`'s frame count, end to
/// end, by 4-point cubic (Catmull-Rom) interpolation per channel.
///
/// The first and last input frames land exactly on the first and last output
/// frames, so consecutive blocks join; `prev` is the frame before `input`,
/// the cubic's left neighbour at the block's start. When the counts match the
/// copy is exact: a block that does not slip is the rendered block.
pub fn respace(input: &[f32], prev: [f32; CHANNELS], out: &mut [f32]) {
    let n_in = input.len() / CHANNELS;
    let n_out = out.len() / CHANNELS;
    if n_in == n_out {
        out.copy_from_slice(input);
        return;
    }
    if n_in == 0 || n_out == 0 {
        out.fill(0.0);
        return;
    }
    let at = |i: isize, c: usize| -> f32 {
        if i < 0 {
            prev[c]
        } else if (i as usize) < n_in {
            input[i as usize * CHANNELS + c]
        } else {
            // Past the end: extrapolate linearly from the last two frames.
            let last = input[(n_in - 1) * CHANNELS + c];
            let before = if n_in > 1 {
                input[(n_in - 2) * CHANNELS + c]
            } else {
                last
            };
            last + (last - before) * (i as usize - (n_in - 1)) as f32
        }
    };
    for j in 0..n_out {
        // From integers, so both ends are exact: j = n_out − 1 lands on
        // n_in − 1 with no rounding left over.
        let p = if n_out > 1 {
            (j * (n_in - 1)) as f64 / (n_out - 1) as f64
        } else {
            0.0
        };
        let i = p.floor() as isize;
        let t = (p - i as f64) as f32;
        for c in 0..CHANNELS {
            let (x0, x1, x2, x3) = (at(i - 1, c), at(i, c), at(i + 1, c), at(i + 2, c));
            out[j * CHANNELS + c] = x1
                + 0.5
                    * t
                    * (x2 - x0
                        + t * (2.0 * x0 - 5.0 * x1 + 4.0 * x2 - x3
                            + t * (3.0 * (x1 - x2) + x3 - x0)));
        }
    }
}
