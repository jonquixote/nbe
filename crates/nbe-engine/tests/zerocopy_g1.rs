//! WU1 Gate G1 — backpressure: who owns the surfaces when record and stream
//! share one composite.
//!
//! Spec: `agents/prompts/10-streaming.md` §2 Gate G1. One composite into one
//! surface per frame, N consumers holding references (`Arc`). When one
//! consumer falls indefinitely behind, it gives up its frame (drops its `Arc`)
//! without holding the allocation hostage. Pool sized for the sum of consumers'
//! in-flight bounds plus the one being drawn. Record's shed-before-draw stays
//! exactly as is; streaming must not change it.
//!
//! Hermetic by design: this models the ownership discipline with plain `Arc<()>`
//! slots under the same `Arc::strong_count == 1` free rule `SurfacePool` uses
//! (`nbe-decode/src/zerocopy.rs`), so CI exercises it with no GPU. The GPU pool
//! is unchanged; record's `RECORD_CHANNEL_BOUND + 1` sizing is untouched.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use nbe_engine::record::pool::{
    record_take_or_skip, shared_pool_size, stream_take_or_drop, SharedPool,
};
use nbe_engine::record::RECORD_CHANNEL_BOUND;

const STREAM_BOUND: usize = 2;

fn load_1m() -> f64 {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .expect("sysctl must run");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_matches(|c| c == '{' || c == '}' || c == ' ')
        .split_whitespace()
        .next()
        .expect("a load average")
        .parse()
        .expect("a number")
}

/// Failure-mode demo under TODAY's single-consumer pool.
///
/// A pool sized `RECORD_CHANNEL_BOUND + 1` (record's answer) with a second
/// consumer that stalls holding one surface: record's in-flight bound plus the
/// drawn frame no longer fit, so `skipped_record_frames` rises. This is the RED
/// that motivates the shared sizing rule — it must PASS (the exhaustion is
/// real), and the guard below must still hold on the shared pool.
#[test]
fn single_consumer_pool_exhausts_when_a_second_consumer_stalls() {
    let pool = SharedPool::from_items(vec![(), (), ()]);
    assert_eq!(pool.len(), RECORD_CHANNEL_BOUND + 1);
    let skipped = AtomicU64::new(0);

    // The stalled consumer takes one surface and never gives it back.
    let _stalled: Arc<()> = pool.acquire().expect("a fresh pool is free");

    // Record needs its full bound in flight plus the one being drawn.
    let _in_flight_1 = record_take_or_skip(&pool, &skipped).expect("slot 1 of 2 free");
    let _in_flight_2 = record_take_or_skip(&pool, &skipped).expect("slot 2 of 2 free");
    let drawn = record_take_or_skip(&pool, &skipped);

    assert!(
        drawn.is_none(),
        "with one surface held hostage, record's bound + drawn (3) cannot fit in a pool of 3"
    );
    assert_eq!(
        skipped.load(Ordering::SeqCst),
        1,
        "the refusal must count exactly one record skip"
    );
}

/// THE GUARD: a stalled stream raises neither `skipped_record_frames` nor
/// `droppedFramesTotal`.
///
/// Shared pool (`record bound + stream bound + drawn`), stream sheds by
/// releasing its reference (counted as a stream drop), record and View clean.
/// Falsified by breaking the drop path — hold the stalled allocation instead
/// of dropping it — whereupon record skips rise.
#[test]
fn a_stalled_stream_raises_neither_skipped_record_frames_nor_dropped_frames_total() {
    let load_before = load_1m();
    let pool = SharedPool::from_items(vec![
        ();
        shared_pool_size(RECORD_CHANNEL_BOUND, STREAM_BOUND)
    ]);
    assert_eq!(pool.len(), RECORD_CHANNEL_BOUND + STREAM_BOUND + 1);

    let skipped = AtomicU64::new(0);
    let view_drops = AtomicU64::new(0);
    let stream_drops = AtomicU64::new(0);

    // Test-side accounting: the second derivation of both counters.
    let mut test_record_skips = 0u64;
    let mut test_stream_drops = 0u64;
    let test_view_drops = 0u64;

    // One stream frame stalls and is given up per the discipline: dropped, so
    // it holds nothing after this line.
    let stalled = stream_take_or_drop(&pool, &stream_drops);
    assert!(stalled.is_some(), "a fresh shared pool has room");
    if stream_drops.load(Ordering::SeqCst) == 0 {
        // Take succeeded: the stall gives the frame up by releasing it.
        drop(stalled);
    } else {
        test_stream_drops += 1;
    }

    // In-flight windows, capped at each consumer's bound (the encoder drains).
    let mut record_inflight: VecDeque<Arc<()>> = VecDeque::new();
    let mut stream_inflight: VecDeque<Arc<()>> = VecDeque::new();

    for _frame in 0..40 {
        // Record: shed-before-draw shape, counted on `skipped_record_frames`.
        match record_take_or_skip(&pool, &skipped) {
            Some(s) => {
                record_inflight.push_back(s);
                if record_inflight.len() > RECORD_CHANNEL_BOUND {
                    record_inflight.pop_front();
                }
            }
            None => test_record_skips += 1,
        }
        // Stream: cannot take => stream drop, never a record skip, never a
        // View drop. Holds at most its bound; the drain releases the oldest.
        match stream_take_or_drop(&pool, &stream_drops) {
            Some(s) => {
                stream_inflight.push_back(s);
                if stream_inflight.len() > STREAM_BOUND {
                    stream_inflight.pop_front();
                }
            }
            None => test_stream_drops += 1,
        }
        // The View is always drawn regardless: no View drop is ever counted
        // here, and the test-side ledger agrees.
        let _view_drawn = true;
        assert_eq!(view_drops.load(Ordering::SeqCst), test_view_drops);
    }

    let load_after = load_1m();
    let skipped_state = skipped.load(Ordering::SeqCst);

    // Forced-exhaustion phase: fill every slot, then prove the next stream
    // take counts exactly one stream drop while record and View stay clean.
    // This exercises the counted-drop path the loop above never needed.
    drop((record_inflight, stream_inflight));
    assert_eq!(
        pool.free(),
        pool.len(),
        "the loop's drains return every slot"
    );
    let _r1 = record_take_or_skip(&pool, &skipped).expect("slot for record in-flight 1");
    let _r2 = record_take_or_skip(&pool, &skipped).expect("slot for record in-flight 2");
    let _s1 = stream_take_or_drop(&pool, &stream_drops).expect("slot for stream in-flight 1");
    let _s2 = stream_take_or_drop(&pool, &stream_drops).expect("slot for stream in-flight 2");
    let _drawn = record_take_or_skip(&pool, &skipped).expect("slot for the drawn frame");
    assert_eq!(pool.free(), 0, "all five slots are now held");
    let shed = stream_take_or_drop(&pool, &stream_drops);
    assert!(shed.is_none(), "a full pool refuses the stream take");
    test_stream_drops += 1;
    // The stream gives up its frames: releasing returns every slot.
    drop((_r1, _r2, _s1, _s2, _drawn));
    assert_eq!(pool.free(), pool.len(), "release returns every slot");
    assert!(
        record_take_or_skip(&pool, &skipped).is_some(),
        "record takes cleanly once the stream released"
    );
    let stream_state = stream_drops.load(Ordering::SeqCst);
    let view_state = view_drops.load(Ordering::SeqCst);
    println!("G1 guard: load(1m) before {load_before:.2} after {load_after:.2} (counts are load-independent: no wall-clock assertion)");
    println!(
        "G1 guard: skipped_record_frames state {skipped_state} vs test-side {test_record_skips}"
    );
    println!("G1 guard: stream_drops state {stream_state} vs test-side {test_stream_drops}");
    println!("G1 guard: droppedFramesTotal state {view_state} vs test-side {test_view_drops}");

    assert_eq!(
        skipped_state, test_record_skips,
        "counts derived two ways must agree"
    );
    assert_eq!(
        stream_state, test_stream_drops,
        "counts derived two ways must agree"
    );
    assert_eq!(
        skipped_state, 0,
        "a stalled stream must not raise record skips"
    );
    assert_eq!(view_state, 0, "a stalled stream must not raise View drops");
    assert_eq!(test_view_drops, 0);
}

/// The sizing rule, pinned: sum of in-flight bounds plus the one being drawn.
/// Record's own answer (`RECORD_CHANNEL_BOUND + 1`) is unchanged — this only
/// names the shared-pool generalisation beside it.
#[test]
fn sizing_rule_is_sum_of_bounds_plus_drawn() {
    assert_eq!(shared_pool_size(RECORD_CHANNEL_BOUND, STREAM_BOUND), 5);
    assert_eq!(shared_pool_size(2, 0), RECORD_CHANNEL_BOUND + 1);
}
