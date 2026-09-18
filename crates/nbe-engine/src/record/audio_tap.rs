//! Master-bus audio tap (Prompt 09 WU34, SPEC §9.3).
//!
//! The audio thread is real-time and dedicated (dress-rehearsal R4); file I/O
//! on it is forbidden. The tap is the boundary: the audio callback copies mix
//! PCM here and returns, and the record thread drains the ring at its own pace
//! into the writer. Nothing in this file touches the filesystem, spawns work,
//! or waits on anything the writer owns.
//!
//! Drop policy (the whole contract, stated once):
//! - The ring is bounded (`capacity` samples, fixed at construction).
//! - `push` never blocks the caller: it uses `try_lock` exactly once. If the
//!   record thread holds the lock mid-drain, the incoming buffer is counted
//!   as dropped and `push` returns — a 5 ms gap in a recording beats a missed
//!   audio deadline on air.
//! - Otherwise the copy lands, and any excess over capacity evicts the OLDEST
//!   samples first (a late writer loses the past, never the present).
//! - Every dropped sample is counted in `dropped()` (lock-free `AtomicU64`);
//!   drops are telemetry, never errors.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// 5 s of stereo 48 kHz mix: the record thread drains far faster than this
/// fills, so the bound only engages on writer stalls, not in steady state.
pub const DEFAULT_CAPACITY_SAMPLES: usize = 48_000 * 2 * 5;

pub struct AudioTap {
    inner: Mutex<VecDeque<f32>>,
    capacity: usize,
    dropped: AtomicU64,
}

impl AudioTap {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY_SAMPLES)
    }

    pub fn with_capacity(capacity_samples: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity_samples.max(1))),
            capacity: capacity_samples.max(1),
            dropped: AtomicU64::new(0),
        }
    }

    /// Copy mix PCM into the ring. Real-time safe by construction: one
    /// `try_lock`, a bounded memcpy into pre-reserved storage, no I/O, no
    /// allocation in the steady state, no waiting.
    ///
    /// Allocation note (audio-thread contract): the ring buffer is reserved
    /// once at construction (`VecDeque::with_capacity`). This method never
    /// lets `len` exceed `capacity` — overfill is evicted BEFORE the copy,
    /// and an input larger than the whole ring keeps only its tail — so the
    /// `extend` under the lock never grows the buffer and performs no
    /// vec-alloc in the steady state. The only work under the lock is the
    /// bounded copy plus bookkeeping; contention sheds via a single
    /// `try_lock` (counted, never waited on).
    pub fn push(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let mut guard = match self.inner.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::WouldBlock) => {
                // The record thread is draining: shed load, count it, return.
                self.dropped
                    .fetch_add(samples.len() as u64, Ordering::Relaxed);
                return;
            }
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        };
        // Evict BEFORE copying so `len` never exceeds `capacity`: the
        // pre-reserved buffer never grows, hence no allocation under lock.
        if samples.len() >= self.capacity {
            let keep = &samples[samples.len() - self.capacity..];
            let dropped = (guard.len() + samples.len() - self.capacity) as u64;
            guard.clear();
            guard.extend(keep.iter().copied());
            self.dropped.fetch_add(dropped, Ordering::Relaxed);
            return;
        }
        if guard.len() + samples.len() > self.capacity {
            let excess = guard.len() + samples.len() - self.capacity;
            guard.drain(..excess);
            self.dropped.fetch_add(excess as u64, Ordering::Relaxed);
        }
        guard.extend(samples.iter().copied());
    }

    /// Take everything buffered. Called on the record thread, never the audio
    /// thread — the allocation this performs is why.
    pub fn drain(&self) -> Vec<f32> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Samples dropped so far: overfill evictions plus contention sheds.
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
