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

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

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
}

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
        }
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
            let head = self.head.load(Ordering::Acquire);
            let n = head.wrapping_sub(tail).min(self.capacity);
            if n == 0 {
                return Vec::new();
            }
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let bits = self.buf[tail.wrapping_add(i) % self.capacity].load(Ordering::Relaxed);
                out.push(f32::from_bits(bits));
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
}
