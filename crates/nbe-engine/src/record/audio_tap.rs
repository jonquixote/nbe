//! Master-bus audio tap (Prompt 09 WU34, SPEC §9.3).
//!
//! The audio thread is real-time and dedicated (dress-rehearsal R4); file I/O
//! on it is forbidden. The tap is the boundary: the audio callback copies mix
//! PCM here and returns, and the record thread drains the ring at its own pace
//! into the writer. Nothing in this file touches the filesystem, spawns work,
//! locks, or waits on anything the writer owns.
//!
//! Drop policy (the whole contract, stated once):
//! - The ring is bounded (`capacity` samples, fixed at construction).
//! - `push` never blocks the caller: it performs no locking, no allocation,
//!   and no I/O — only atomic loads/stores plus a bounded memcpy into
//!   pre-reserved storage. A 5 ms gap in a recording beats a missed audio
//!   deadline on air, but with this design there is no gap at all in steady
//!   state: producer and consumer never contend on a lock.
//! - Otherwise the copy lands, and any excess over capacity evicts the OLDEST
//!   samples first (a late writer loses the past, never the present).
//! - **Eviction is in whole stereo frames, so a drain never re-pairs the
//!   channels.** The ring holds interleaved stereo ([`CHANNELS`] samples per
//!   frame); its capacity is a whole number of frames, `push` stores and
//!   evicts only whole frames, so `head` and `tail` only ever move by whole
//!   frames and every drain starts on a left sample and holds whole frames. A
//!   stalled consumer loses its oldest frames, never its L/R pairing. A
//!   trailing odd sample handed to `push` is not stored (it would shift every
//!   later frame) and is counted dropped.
//!
//!   ~~(no such line)~~ — before PR #30's final repair, eviction was per
//!   sample: a drain racing an eviction mid-push could return an odd count
//!   starting on a right sample (a two-key-pass probe measured 177 of 907
//!   drains under a constantly full ring). The record writer refused such a
//!   drain as "audio must be whole stereo frames" and the take ended; the
//!   stream thread re-paired it from the shifted start and swapped L and R
//!   silently until the next odd drain.
//! - Every dropped sample is counted in `dropped()` (lock-free `AtomicU64`);
//!   drops are telemetry, never errors.
//!
//! Concurrency contract (SPSC): exactly ONE producer (the audio thread calls
//! `push`) and ONE consumer (the record thread calls `drain`/`clear`/`len`).
//! `dropped()` may be read from any thread. `head` is producer-owned (the
//! consumer only loads it); `tail` is advanced by the consumer on drain and
//! by the producer on overflow-eviction via CAS, so a stall-then-burst never
//! tears. Multi-producer use is memory-safe (all shared state is atomic, all
//! indices are bounds-checked) but outside the contract: concurrent `push`
//! calls may overwrite each other's slots.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Samples per frame: the master mix is interleaved stereo.
pub const CHANNELS: usize = 2;

/// 5 s of stereo 48 kHz mix: the record thread drains far faster than this
/// fills, so the bound only engages on writer stalls, not in steady state.
pub const DEFAULT_CAPACITY_SAMPLES: usize = 48_000 * 2 * 5;

pub struct AudioTap {
    /// Preallocated sample slots as raw `f32` bits. Sized once at
    /// construction; neither path allocates after that.
    buf: Box<[AtomicU32]>,
    capacity: usize,
    /// Monotonic producer write counter. Only `push` stores it; the consumer
    /// loads it. Slot index is `head % capacity`.
    head: AtomicUsize,
    /// Monotonic read counter. Advanced by the consumer (`drain`/`clear`) and
    /// by the producer when evicting the oldest on overflow (CAS in both
    /// directions, so neither tears the other's advance).
    tail: AtomicUsize,
    dropped: AtomicU64,
    /// Where the positioned pushes start: one mark for the first
    /// [`Self::push_at`] and one for every later one that does not continue
    /// the push before it (`MARKS` kept, a ring). A mark says the frame at
    /// sample counter `counter` is master frame `pos`; the frames after it,
    /// up to the next mark, follow it one by one. So every drained frame is
    /// placed where it was pushed, however the producer jumped. (It used to
    /// be one origin, fixed by the first push: CI's null sink repeated master
    /// frame 0 for the blocks due before a show's clock started, and every
    /// later frame of that take was placed late by the repeats.)
    marks: Box<[Mark]>,
    /// Marks written, monotonic; producer-owned (the consumer loads it).
    marks_written: AtomicUsize,
    /// The master frame just past the last positioned push; [`NO_POS`]
    /// before one. Producer-owned.
    end: AtomicI64,
    /// `push_at` calls whose position did not continue the previous push
    /// (every mark but the first). Zero in a steady take; the record thread
    /// reports it.
    discontinuities: AtomicU64,
}

/// One placement: sample counter `counter` is master frame `pos`.
struct Mark {
    counter: AtomicUsize,
    pos: AtomicI64,
}

/// Marks kept. A drain needs only the marks of the samples it takes, which are
/// at most the ring's 5 s; at one mark per 1,600-frame block that is 150.
const MARKS: usize = 512;

/// [`AudioTap::end`] before any positioned push.
const NO_POS: i64 = i64::MIN;

impl AudioTap {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY_SAMPLES)
    }

    /// A ring of `capacity_samples`, rounded down to whole stereo frames
    /// (at least one frame).
    pub fn with_capacity(capacity_samples: usize) -> Self {
        let capacity = (capacity_samples - capacity_samples % CHANNELS).max(CHANNELS);
        // Safe construction without MaybeUninit: one bulk fill at birth,
        // never reallocated afterwards.
        let mut buf = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            buf.push(AtomicU32::new(0.0f32.to_bits()));
        }
        Self {
            buf: buf.into_boxed_slice(),
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            marks: (0..MARKS)
                .map(|_| Mark {
                    counter: AtomicUsize::new(0),
                    pos: AtomicI64::new(0),
                })
                .collect(),
            marks_written: AtomicUsize::new(0),
            end: AtomicI64::new(NO_POS),
            discontinuities: AtomicU64::new(0),
        }
    }

    /// [`Self::push`], placing the block on the master clock: `pos` is the
    /// master frame of its first sample. Real-time safe, like `push` (atomics
    /// only, into preallocated marks). A push whose `pos` does not continue
    /// the previous positioned push is marked where it lands, and counted in
    /// [`Self::discontinuities`]: its frames are placed at `pos`, not after
    /// the frames before it.
    pub fn push_at(&self, samples: &[f32], pos: i64) {
        // Producer-owned, both: one producer (the module contract).
        let head = self.head.load(Ordering::Relaxed);
        let end = self.end.load(Ordering::Relaxed);
        if end != pos {
            let m = self.marks_written.load(Ordering::Relaxed);
            let mark = &self.marks[m % MARKS];
            mark.counter.store(head, Ordering::Relaxed);
            mark.pos.store(pos, Ordering::Relaxed);
            // Published before `push` publishes `head`: a consumer that sees
            // the new samples sees their mark.
            self.marks_written.store(m + 1, Ordering::Release);
            if end != NO_POS {
                self.discontinuities.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.push(samples);
        let frames = (samples.len() / CHANNELS) as i64;
        self.end.store(pos + frames, Ordering::Release);
    }

    /// Like [`Self::drain`], split where the positioned pushes jumped: each
    /// segment is the master frame of its first frame (`None` for samples
    /// pushed before any positioned push) and its samples, in order.
    pub fn drain_segments(&self) -> Vec<(Option<i64>, Vec<f32>)> {
        loop {
            let tail = self.tail.load(Ordering::Acquire);
            let Some(out) = self.drain_from(tail) else {
                continue;
            };
            if out.is_empty() {
                return Vec::new();
            }
            // After the drain: the head it read published every mark of the
            // samples it took.
            return self.split(tail, out);
        }
    }

    /// Cut `out` (the samples from counter `tail`) at the marks inside it.
    fn split(&self, tail: usize, mut out: Vec<f32>) -> Vec<(Option<i64>, Vec<f32>)> {
        let written = self.marks_written.load(Ordering::Acquire);
        let kept = written.saturating_sub(MARKS)..written;
        let end = tail.wrapping_add(out.len());
        // The marks inside (tail, end), in order, the last of any that share a
        // counter (a push that stored nothing).
        let mut cuts: Vec<(usize, i64)> = Vec::new();
        for i in kept.clone() {
            let mark = &self.marks[i % MARKS];
            let counter = mark.counter.load(Ordering::Relaxed);
            let pos = mark.pos.load(Ordering::Relaxed);
            if counter > tail && counter < end {
                match cuts.last_mut() {
                    Some(last) if last.0 == counter => last.1 = pos,
                    _ => cuts.push((counter, pos)),
                }
            }
        }
        let mut segments = Vec::with_capacity(cuts.len() + 1);
        let mut at = self.position_in(kept, tail);
        let mut from = tail;
        for (counter, pos) in cuts {
            let rest = out.split_off(counter - from);
            segments.push((at, std::mem::replace(&mut out, rest)));
            from = counter;
            at = Some(pos);
        }
        segments.push((at, out));
        segments
    }

    /// The master frame at sample counter `counter`, from the newest kept mark
    /// at or before it; `None` before the first positioned push.
    fn position_in(&self, kept: std::ops::Range<usize>, counter: usize) -> Option<i64> {
        for i in kept.rev() {
            let mark = &self.marks[i % MARKS];
            let at = mark.counter.load(Ordering::Relaxed);
            if at <= counter {
                return Some(mark.pos.load(Ordering::Relaxed) + ((counter - at) / CHANNELS) as i64);
            }
        }
        None
    }

    /// The master frame just past the last frame pushed (`None` when the tap
    /// is not on the clock): what the record thread waits on at a stop.
    pub fn end_position(&self) -> Option<i64> {
        let end = self.end.load(Ordering::Acquire);
        (end != NO_POS).then_some(end)
    }

    /// Positioned pushes that did not continue the previous one.
    pub fn discontinuities(&self) -> u64 {
        self.discontinuities.load(Ordering::Relaxed)
    }

    /// One drain attempt starting at `tail`: `None` if the producer evicted
    /// concurrently (the caller retakes the snapshot), else the samples.
    fn drain_from(&self, tail: usize) -> Option<Vec<f32>> {
        let head = self.head.load(Ordering::Acquire);
        let n = head.wrapping_sub(tail).min(self.capacity);
        if n == 0 {
            return Some(Vec::new());
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let bits = self.buf[tail.wrapping_add(i) % self.capacity].load(Ordering::Relaxed);
            out.push(f32::from_bits(bits));
        }
        self.tail
            .compare_exchange_weak(
                tail,
                tail.wrapping_add(n),
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_ok()
            .then_some(out)
    }

    /// Samples the ring holds: `capacity` fixed at construction.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Copy mix PCM into the ring. Real-time safe by construction: no locks,
    /// no allocation, no I/O. Only atomic index traffic plus the bounded copy
    /// into pre-reserved storage.
    ///
    /// Ordering: the `tail` load is `Acquire` (observe the consumer's frees);
    /// the sample stores are `Relaxed` but sequenced before the `head`
    /// `Release` publish, which the consumer reads with `Acquire` — so every
    /// drained sample is fully written. `dropped` is `Relaxed` telemetry, as
    /// before.
    pub fn push(&self, samples: &[f32]) {
        // Whole frames only (see the module contract): a trailing odd sample
        // is counted dropped, never stored.
        let whole = samples.len() - samples.len() % CHANNELS;
        if whole < samples.len() {
            self.dropped
                .fetch_add((samples.len() - whole) as u64, Ordering::Relaxed);
        }
        if whole == 0 {
            return;
        }
        // Producer-owned: no other thread stores `head`, so one load at entry
        // plus one publish at exit brackets the whole block.
        let mut head = self.head.load(Ordering::Relaxed);
        for frame in samples[..whole].as_chunks::<CHANNELS>().0 {
            // Make room for one whole frame: while full, evict the oldest whole
            // frame (advance `tail` by CHANNELS, count it). The CAS retries
            // only if the consumer advanced `tail` concurrently — forward
            // progress either way. `head` and `tail` stay multiples of
            // CHANNELS, so no drain can start mid-frame.
            loop {
                let tail = self.tail.load(Ordering::Acquire);
                // `capacity - CHANNELS`, never `occupancy + CHANNELS`: under
                // multi-producer misuse a stale `head` wraps the occupancy to a
                // huge value, and adding to it would overflow (a debug panic on
                // the audio thread). Capacity is at least one frame.
                if head.wrapping_sub(tail) <= self.capacity - CHANNELS {
                    break;
                }
                if self
                    .tail
                    .compare_exchange_weak(
                        tail,
                        tail.wrapping_add(CHANNELS),
                        Ordering::Release,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    self.dropped.fetch_add(CHANNELS as u64, Ordering::Relaxed);
                    break;
                }
            }
            for &s in frame {
                self.buf[head % self.capacity].store(s.to_bits(), Ordering::Relaxed);
                head = head.wrapping_add(1);
            }
        }
        self.head.store(head, Ordering::Release);
    }

    /// Take everything buffered. Called on the record thread, never the audio
    /// thread — the allocation this performs is why.
    ///
    /// The `tail` CAS retries only if the producer evicted concurrently (a
    /// seconds-stalled consumer racing fresh input); the snapshot is then
    /// retaken, so no sample is returned twice or skipped silently — an
    /// evicted sample is counted in `dropped()`, never duplicated out.
    pub fn drain(&self) -> Vec<f32> {
        loop {
            let tail = self.tail.load(Ordering::Acquire);
            if let Some(out) = self.drain_from(tail) {
                return out;
            }
        }
    }

    /// Discard everything buffered, counting it as dropped. Consumer-side
    /// (record thread), like `drain` but without the copy.
    pub fn clear(&self) {
        loop {
            let tail = self.tail.load(Ordering::Acquire);
            let head = self.head.load(Ordering::Acquire);
            let n = head.wrapping_sub(tail).min(self.capacity);
            if n == 0 {
                return;
            }
            if self
                .tail
                .compare_exchange_weak(
                    tail,
                    tail.wrapping_add(n),
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                self.dropped.fetch_add(n as u64, Ordering::Relaxed);
                return;
            }
        }
    }

    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail).min(self.capacity)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Samples dropped so far: overfill evictions (plus `clear()` discards).
    /// With no lock left to collide on, contention sheds are gone by
    /// construction — overflow from a seconds-stalled consumer still counts.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Default for AudioTap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // ~~`push(&[1.0, 2.0, 3.0])` into a capacity-4 ring~~ — these two tests
    // pushed 3-sample (half-frame) blocks, a shape the stereo mix never
    // produces and whole-frame eviction no longer stores. Same properties,
    // in frames.

    #[test]
    fn overfill_drops_oldest_frames_and_counts() {
        let tap = AudioTap::with_capacity(4);
        tap.push(&[1.0, 2.0]);
        assert_eq!(tap.dropped(), 0);
        tap.push(&[3.0, 4.0, 5.0, 6.0]);
        assert_eq!(tap.dropped(), 2, "one whole frame evicted");
        assert_eq!(tap.drain(), vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn single_push_larger_than_capacity_keeps_the_tail() {
        let tap = AudioTap::with_capacity(4);
        tap.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(tap.dropped(), 2);
        assert_eq!(tap.drain(), vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn capacity_is_whole_frames_and_odd_trailers_are_counted_not_stored() {
        assert_eq!(AudioTap::with_capacity(5).capacity(), 4);
        assert_eq!(AudioTap::with_capacity(1).capacity(), CHANNELS);
        let tap = AudioTap::with_capacity(8);
        tap.push(&[1.0, 2.0, 3.0]);
        assert_eq!(tap.dropped(), 1, "the trailing half-frame is counted");
        assert_eq!(tap.drain(), vec![1.0, 2.0]);
    }

    /// The two-key pass's probe, kept as the guard: a producer pushing
    /// stereo frames (L = +1.0, R = -1.0) into a constantly full ring while a
    /// consumer drains concurrently. Every drain must hold whole frames and
    /// start on a left sample. With per-sample eviction this measured 177
    /// odd-length drains of 907, all starting on a right sample.
    #[test]
    fn drains_racing_eviction_stay_stereo_aligned() {
        let tap = Arc::new(AudioTap::with_capacity(64));
        let stop = Arc::new(AtomicBool::new(false));
        let (t2, s2) = (tap.clone(), stop.clone());
        let producer = std::thread::spawn(move || {
            let mut block = [0.0f32; 16];
            for (i, s) in block.iter_mut().enumerate() {
                *s = if i % 2 == 0 { 1.0 } else { -1.0 };
            }
            while !s2.load(Ordering::SeqCst) {
                t2.push(&block);
            }
        });
        let (mut drains, mut odd, mut starts_on_r) = (0u64, 0u64, 0u64);
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(1500) {
            let d = tap.drain();
            if !d.is_empty() {
                drains += 1;
                odd += (d.len() % 2) as u64;
                starts_on_r += u64::from(d[0] < 0.0);
            }
            if drains % 64 == 0 {
                std::thread::sleep(Duration::from_micros(20));
            }
        }
        stop.store(true, Ordering::SeqCst);
        producer.join().unwrap();
        eprintln!(
            "STEREO GUARD: {drains} drains racing {} evicted samples: {odd} odd-length, \
             {starts_on_r} starting on a right-channel sample",
            tap.dropped()
        );
        assert!(
            drains > 0 && tap.dropped() > 0,
            "the ring must have been full and drained"
        );
        assert_eq!(odd, 0, "a drain must hold whole stereo frames");
        assert_eq!(
            starts_on_r, 0,
            "a drain must start on a left-channel sample"
        );
    }

    /// One segment's position and frame count.
    fn placed(segments: &[(Option<i64>, Vec<f32>)]) -> Vec<(Option<i64>, usize)> {
        segments
            .iter()
            .map(|(p, s)| (*p, s.len() / CHANNELS))
            .collect()
    }

    #[test]
    fn positioned_pushes_place_every_drained_frame_on_the_master_clock() {
        let tap = AudioTap::with_capacity(100);
        assert_eq!(
            tap.end_position(),
            None,
            "a tap no push placed has no position"
        );
        tap.push_at(&[0.1; 8], 1_000);
        assert_eq!(tap.end_position(), Some(1_004));
        assert_eq!(placed(&tap.drain_segments()), vec![(Some(1_000), 4)]);
        tap.push_at(&[0.2; 6], 1_004);
        assert_eq!(
            placed(&tap.drain_segments()),
            vec![(Some(1_004), 3)],
            "the next drain starts where the last ended"
        );
        assert_eq!(tap.discontinuities(), 0);
    }

    #[test]
    fn an_eviction_moves_the_drained_position_with_the_frames_it_dropped() {
        // Capacity 4 frames; 6 pushed: the 2 oldest evicted.
        let tap = AudioTap::with_capacity(8);
        tap.push_at(&[0.5; 12], 50);
        assert_eq!(placed(&tap.drain_segments()), vec![(Some(52), 4)]);
    }

    #[test]
    fn a_push_that_does_not_continue_the_last_is_counted() {
        let tap = AudioTap::with_capacity(100);
        tap.push_at(&[0.0; 4], 0);
        tap.push_at(&[0.0; 4], 2);
        assert_eq!(tap.discontinuities(), 0);
        tap.push_at(&[0.0; 4], 10);
        assert_eq!(
            tap.discontinuities(),
            1,
            "a jump in position is counted, not hidden"
        );
    }

    #[test]
    fn a_jump_splits_the_drain_and_each_part_keeps_where_it_was_pushed() {
        let tap = AudioTap::with_capacity(100);
        tap.push_at(&[0.1; 4], 0); // frames 0-1
        tap.push_at(&[0.2; 4], 0); // frames 0-1 again: a repeat
        tap.push_at(&[0.3; 4], 2); // continues the repeat
        tap.push_at(&[0.4; 4], 9); // a gap
        let segments = tap.drain_segments();
        assert_eq!(
            placed(&segments),
            vec![(Some(0), 2), (Some(0), 4), (Some(9), 2)]
        );
        assert_eq!(segments[1].1, vec![0.2, 0.2, 0.2, 0.2, 0.3, 0.3, 0.3, 0.3]);
        assert_eq!(tap.end_position(), Some(11));
    }

    #[test]
    fn a_jump_inside_an_eviction_still_places_what_is_kept() {
        // Capacity 4 frames. 2 frames at 100, then 4 at 500: the first push
        // is evicted whole, and the drain starts at the second push's mark.
        let tap = AudioTap::with_capacity(8);
        tap.push_at(&[0.1; 4], 100);
        tap.push_at(&[0.2; 8], 500);
        assert_eq!(placed(&tap.drain_segments()), vec![(Some(500), 4)]);
    }

    #[test]
    fn unpositioned_samples_drain_unplaced() {
        let tap = AudioTap::with_capacity(100);
        tap.push(&[0.1; 4]);
        assert_eq!(placed(&tap.drain_segments()), vec![(None, 2)]);
        assert_eq!(tap.end_position(), None);
    }
}
