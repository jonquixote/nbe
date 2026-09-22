//! Gate G1 — shared-surface pool ownership (`agents/prompts/10-streaming.md` §2).
//!
//! ## Sizing rule
//!
//! One composite into one surface per frame, N consumers holding references.
//! The pool holds the **sum of its consumers' in-flight bounds plus the one
//! being drawn**: `record bound + stream bound + 1`. Record's own answer,
//! `RECORD_CHANNEL_BOUND + 1`, is unchanged and stays where it is
//! ([`crate::record::zerocopy_pool`]); this module only names the shared
//! generalisation beside it.
//!
//! ## Drop discipline
//!
//! When one consumer falls indefinitely behind, it gives up its frame by
//! **dropping its `Arc`** — never by making the pool wait, never by blocking
//! the draw. A stream frame that cannot be taken is a *stream* drop, counted
//! on the stream counter, and must never become a record skip or a View drop
//! (AC-10 item 4). Record's shed-before-draw (`feed.rs`) is untouched.
//!
//! ## Free rule
//!
//! Same as [`nbe_decode::zerocopy::SurfacePool`]: a slot is free exactly when
//! `Arc::strong_count == 1`. The pool hands out the only clones and
//! [`SharedPool::acquire`] runs on the loop, so the count can only fall
//! asynchronously as consumers drop what they finished with. One-sided by
//! construction: may read busy for a surface freed a microsecond ago (costs a
//! skip), never hands out a surface still in flight. This module models the
//! discipline generically (no GPU) so the guard runs hermetically; the GPU
//! pool keeps its own implementation.
//!
//! thiserror for library errors: this module has no fallible constructors that
//! need a new error type — an empty pool is a programming error and panics at
//! construction, the same way a zero-surface `SurfacePool::new` refuses loudly
//! rather than building a pool that can never hand anything out.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The one being drawn: the `+ 1` in every sizing rule.
pub const DRAWN_ONE: usize = 1;

/// Shared-pool size: sum of consumers' in-flight bounds plus the drawn one.
pub fn shared_pool_size(record_bound: usize, stream_bound: usize) -> usize {
    record_bound + stream_bound + DRAWN_ONE
}

/// N slots under the `strong_count == 1` free rule. The pool holds the only
/// long-lived `Arc`s; every hand-out is a clone, every return is a drop.
#[derive(Debug)]
pub struct SharedPool<T> {
    slots: Vec<Arc<T>>,
}

impl<T> SharedPool<T> {
    /// Build from one item per slot. Panics on empty: a pool with no slots is
    /// not a pool, and a silent zero would shed every frame it is asked for.
    pub fn from_items(items: Vec<T>) -> Self {
        assert!(
            !items.is_empty(),
            "a shared pool of zero slots is not a pool"
        );
        Self {
            slots: items.into_iter().map(Arc::new).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// How many slots nobody else is holding right now.
    pub fn free(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| Arc::strong_count(s) == 1)
            .count()
    }

    /// Take a free slot, or `None` when every one is still in flight. Never
    /// hands out a slot another holder still has.
    pub fn acquire(&self) -> Option<Arc<T>> {
        self.slots
            .iter()
            .find(|s| Arc::strong_count(s) == 1)
            .map(Arc::clone)
    }
}

/// Record take-or-skip: the generic shape of `acquire_record_surface`
/// (`feed.rs`), counting a refusal on `skipped` for the loop's
/// `skipped_record_frames`. Record's own path is untouched; this exists so the
/// G1 guard derives record pressure the same way under test.
pub fn record_take_or_skip<T>(pool: &SharedPool<T>, skipped: &AtomicU64) -> Option<Arc<T>> {
    match pool.acquire() {
        Some(s) => Some(s),
        None => {
            skipped.fetch_add(1, Ordering::SeqCst);
            None
        }
    }
}

/// Stream take-or-drop: `None` counts **one stream drop** and holds nothing.
/// Never touches the record skip counter, never touches View drops, never
/// blocks the draw — the frame is simply not taken. Breaking this (holding the
/// allocation instead of dropping it) is the falsification: record skips rise.
pub fn stream_take_or_drop<T>(pool: &SharedPool<T>, stream_drops: &AtomicU64) -> Option<Arc<T>> {
    match pool.acquire() {
        Some(s) => Some(s),
        None => {
            stream_drops.fetch_add(1, Ordering::SeqCst);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pool_is_a_programming_error() {
        let survived = std::panic::catch_unwind(|| SharedPool::<()>::from_items(vec![]));
        assert!(survived.is_err(), "zero slots must refuse loudly");
    }

    #[test]
    fn acquire_never_hands_out_a_slot_still_in_flight() {
        let pool = SharedPool::from_items(vec![(), ()]);
        let _held = pool.acquire().expect("first slot free");
        let _held2 = pool.acquire().expect("second slot free");
        assert!(pool.acquire().is_none(), "a full pool refuses");
        assert_eq!(pool.free(), 0);
    }

    #[test]
    fn dropping_returns_the_slot() {
        let pool = SharedPool::from_items(vec![()]);
        let held = pool.acquire().expect("free");
        assert_eq!(pool.free(), 0);
        drop(held);
        assert_eq!(pool.free(), 1);
    }
}
