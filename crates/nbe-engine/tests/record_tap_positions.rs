//! The record tap places every block the null-sink driver pushes where the
//! driver rendered it, including blocks rendered for instants before the
//! show's clock started.
//!
//! Found on CI (#187's run 44886656444, step 12b's take): every one of the
//! take's 194 blocks was counted as a discontinuity. The null-sink driver
//! renders each block at the master frame due at its deadline, and that frame
//! was read with a saturating subtraction from the clock's epoch, so a block
//! whose deadline fell before the epoch (the driver a few blocks behind the
//! wall clock when the show started) was rendered at frame 0. The first
//! blocks of the run were frame 0's audio, repeated. The tap placed
//! everything from its first push, so every later block was placed that many
//! blocks late, and the take's audio cut (`record/align.rs`) cut the wrong
//! samples.
//!
//! Deterministic: no driver task. The driver advances only inside this
//! test's calls to `render_due`, at instants the test chooses relative to the
//! clock's own epoch.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::audio::{BusId, Source};
use nbe_engine::audio_driver::{render_due, AudioDriver, AudioSink};
use nbe_engine::record::AudioTap;
use nbe_engine::state::EngineState;

const HOUSE_RATE: u32 = 30;
/// Frames per block (and per master frame) at 48 kHz and house rate 30.
const BLOCK: i64 = 48_000 / HOUSE_RATE as i64;

struct Discard;

impl AudioSink for Discard {
    fn write(&mut self, _block: &[f32]) -> bool {
        true
    }
    fn block_frames(&self) -> usize {
        BLOCK as usize
    }
}

/// A running clock, a driver with a tone on the mic bus, and a positioned tap
/// attached before the first block. The driver's schedule starts `behind`
/// before the clock's epoch.
fn rig(
    behind: Duration,
) -> (
    Arc<EngineState>,
    AudioDriver,
    Arc<AudioTap>,
    Instant,
    Duration,
) {
    let state = Arc::new(EngineState::new(HOUSE_RATE));
    state.clock.lock().unwrap().start();
    let epoch = state
        .clock
        .lock()
        .unwrap()
        .epoch()
        .expect("a started clock has an epoch");
    let mut driver = AudioDriver::new(state.clone(), Box::new(Discard), HOUSE_RATE);
    driver.graph.set_source(
        BusId::Mic,
        vec![Source::Tone {
            hz: 997.0,
            amplitude: 0.5,
        }],
    );
    let tap = Arc::new(AudioTap::new());
    *state.record_tap.lock().unwrap() = Some(tap.clone());
    let block = driver.block_duration();
    let next = epoch
        .checked_sub(behind)
        .expect("an instant before the epoch");
    (state, driver, tap, next, block)
}

#[test]
fn blocks_due_before_the_clock_started_continue_into_the_run_without_repeating_frame_0() {
    // Three and a half blocks behind the epoch: the first four blocks are due
    // before the show's clock started.
    let (state, mut driver, tap, mut next, block) = rig(block_behind(3.5));
    for _ in 0..12 {
        let now = next;
        render_due(&mut driver, &state, &mut next, block, now);
    }
    assert_eq!(
        tap.discontinuities(),
        0,
        "every block continues the one before it, across the clock's start"
    );
    let segments = tap.drain_segments();
    assert_eq!(
        segments.len(),
        1,
        "one contiguous run: {:?}",
        lens(&segments)
    );
    let (first, samples) = &segments[0];
    assert_eq!(samples.len() as i64, 12 * BLOCK * 2, "12 blocks of stereo");
    assert_eq!(
        *first,
        Some(-4 * BLOCK),
        "the first block is master frame -4's (due 3.5 blocks before the epoch), not frame 0's"
    );
}

#[test]
fn a_push_that_jumps_is_placed_where_it_was_pushed() {
    // The tap itself, whatever the producer does: a repeat, then a gap.
    let tap = AudioTap::new();
    let block = vec![0.25f32; (BLOCK * 2) as usize];
    tap.push_at(&block, 0);
    tap.push_at(&block, 0); // frame 0 again
    tap.push_at(&block, BLOCK); // continues the repeat
    tap.push_at(&block, 5 * BLOCK); // a gap of three blocks
    let segments = tap.drain_segments();
    assert_eq!(
        lens(&segments),
        vec![
            (Some(0), BLOCK as usize),
            (Some(0), 2 * BLOCK as usize),
            (Some(5 * BLOCK), BLOCK as usize)
        ],
        "each run of contiguous pushes keeps the position it was pushed at"
    );
    assert_eq!(tap.discontinuities(), 2, "the repeat and the gap");
    assert_eq!(
        tap.end_position(),
        Some(6 * BLOCK),
        "the end is the last push's"
    );
}

fn block_behind(blocks: f64) -> Duration {
    Duration::from_secs_f64(blocks * BLOCK as f64 / 48_000.0)
}

fn lens(segments: &[(Option<i64>, Vec<f32>)]) -> Vec<(Option<i64>, usize)> {
    segments.iter().map(|(p, s)| (*p, s.len() / 2)).collect()
}
