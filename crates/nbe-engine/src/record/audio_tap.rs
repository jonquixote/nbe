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

    pub fn with_capacity(capacity_samples: usize) -> Self {
        let capacity = capacity_samples.max(1);
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
        if samples.is_empty() {
            return;
        }
        // Producer-owned: no other thread stores `head`, so one load at entry
        // plus one publish at exit brackets the whole block.
        let mut head = self.head.load(Ordering::Relaxed);
        for &s in samples {
            // Make room: while full, evict the oldest (advance `tail`, count
            // it). The CAS retries only if the consumer advanced `tail`
            // concurrently — forward progress either way.
            loop {
                let tail = self.tail.load(Ordering::Acquire);
                if head.wrapping_sub(tail) < self.capacity {
                    break;
                }
                if self
                    .tail
                    .compare_exchange_weak(
                        tail,
                        tail.wrapping_add(1),
                        Ordering::Release,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
            self.buf[head % self.capacity].store(s.to_bits(), Ordering::Relaxed);
            head = head.wrapping_add(1);
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

    #[test]
    fn overfill_drops_oldest_and_counts() {
        let tap = AudioTap::with_capacity(4);
        tap.push(&[1.0, 2.0, 3.0]);
        assert_eq!(tap.dropped(), 0);
        tap.push(&[4.0, 5.0, 6.0]);
        assert_eq!(tap.dropped(), 2);
        assert_eq!(tap.drain(), vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn single_push_larger_than_capacity_keeps_the_tail() {
        let tap = AudioTap::with_capacity(4);
        tap.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(tap.dropped(), 2);
        assert_eq!(tap.drain(), vec![3.0, 4.0, 5.0, 6.0]);
    }
}
