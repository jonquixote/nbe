//! Gate G1 — shared-surface pool sizing (`agents/prompts/10-streaming.md` §2).
//!
//! ## Sizing rule
//!
//! One composite into one surface per frame, N consumers holding references.
//! A consumer holds, at worst, every frame its channel can queue **plus the
//! one its thread is encoding** — and "encoding" lasts until VideoToolbox
//! releases the buffer, which is after `encode_pixel_buffer` returns (see
//! [`nbe_decode::zerocopy::SurfacePool`]'s free rule). The pool holds the
//! **sum of its consumers' in-flight bounds plus the one being drawn**:
//!
//! ```text
//! shared  = (RECORD_CHANNEL_BOUND + 1) + (STREAM_CHANNEL_BOUND + 1) + 1
//! stream  = (STREAM_CHANNEL_BOUND + 1) + 1
//! ```
//!
//! Sum, not max, although both-live consumers share one composite: a frame
//! one consumer shed can still be held by the other, so in the worst case
//! the two hold disjoint frames.
//!
//! PR #30's first rule was `record bound + stream bound + 1` — it left out
//! each thread's in-encode surface, so a both-live take ran two surfaces
//! short and a merely busy stream cost record pre-draw skips.
//!
//! ## Drop discipline
//!
//! When one consumer falls indefinitely behind, it gives up its frame by
//! **dropping its `Arc`** — never by making the pool wait, never by blocking
//! the draw. A stream frame that cannot be taken is a *stream* drop, counted
//! on the stream counter, and must never become a record skip or a View drop
//! (AC-10 item 4). Record's shed-before-draw (`feed.rs`) is untouched. The
//! guard (`tests/zerocopy_g1.rs`) runs these rules on real surfaces through
//! the loop's own tick.

/// The one being drawn: the `+ 1` in every sizing rule.
pub const DRAWN_ONE: usize = 1;

/// The surface a consumer thread is encoding (held until VideoToolbox
/// releases it, not merely until the encode call returns).
pub const IN_ENCODER: usize = 1;

/// One consumer's worst-case hold: its queue plus its encoder.
pub fn consumer_in_flight(channel_bound: usize) -> usize {
    channel_bound + IN_ENCODER
}

/// Shared-pool size: both consumers' in-flight bounds plus the drawn one.
pub fn shared_pool_size(record_bound: usize, stream_bound: usize) -> usize {
    consumer_in_flight(record_bound) + consumer_in_flight(stream_bound) + DRAWN_ONE
}

/// The stream's own pool (stream-only takes, and frames the record loan does
/// not cover): the stream's in-flight bound plus the drawn one.
pub fn stream_pool_size(stream_bound: usize) -> usize {
    consumer_in_flight(stream_bound) + DRAWN_ONE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_count_every_holder() {
        assert_eq!(consumer_in_flight(2), 3);
        assert_eq!(shared_pool_size(2, 2), 7);
        assert_eq!(stream_pool_size(2), 4);
    }
}
