//! A take's audio, cut to its video's span on the master clock.
//!
//! The record tap carries what the audio driver pushed, in the order the
//! driver processed it: a driver behind at the tap's attach puts blocks from
//! before the take into it, and one behind at the detach leaves the take's
//! last blocks out (`tests/record_tap_boundary.rs`). Every block the graph
//! pushes is placed on the master clock (`AudioTap::push_at`), so the record
//! thread can instead take exactly the master frames its video covers:
//! `[first video frame, last video frame + 1)`, in master audio frames. What
//! the tap holds before that span is trimmed; a gap, or audio the tap never
//! got, is filled with silence. The video and audio tracks then cover the same
//! span of the show, whatever the driver's lateness was.
//!
//! Pure bookkeeping: no I/O, no clock. The record thread feeds it drained
//! blocks (`accept`) and takes audio out up to the end of the latest video
//! frame (`take_until`); at the stop it takes the rest and pads (`finish`).

use crate::record::audio_tap::CHANNELS;

/// What [`TakeAudio`] did to fit the take's audio to its video, in master
/// audio frames.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AlignStats {
    /// Tap audio from before the take's first video frame, dropped.
    pub trimmed_head: u64,
    /// Silence put where the tap had nothing, inside the take.
    pub padded: u64,
    /// Silence put at the end, where the tap had not reached the take's end.
    pub padded_tail: u64,
    /// Tap audio from after the take's last video frame, dropped.
    pub trimmed_tail: u64,
    /// Tap audio that overlapped audio already taken (a position that went
    /// backwards), dropped.
    pub overlapped: u64,
}

/// The take's audio from master frame `start` on, as one contiguous run.
#[derive(Debug)]
pub struct TakeAudio {
    /// Master frame of `pending[0]` (the next frame `take_until` hands out).
    cursor: i64,
    /// Interleaved samples from `cursor` onwards, contiguous.
    pending: Vec<f32>,
    stats: AlignStats,
    /// Whether any audio at or past the start has been taken in yet: before
    /// it, audio behind the cursor is the head trimmed; after, an overlap.
    started: bool,
}

impl TakeAudio {
    /// A take whose first video frame starts at master audio frame `start`.
    pub fn new(start: i64) -> Self {
        Self {
            cursor: start,
            pending: Vec::new(),
            stats: AlignStats::default(),
            started: false,
        }
    }

    /// Drained tap audio whose first frame is master frame `pos`.
    pub fn accept(&mut self, pos: i64, samples: &[f32]) {
        let frames = (samples.len() / CHANNELS) as i64;
        if frames == 0 {
            return;
        }
        let end = self.cursor + (self.pending.len() / CHANNELS) as i64;
        let mut skip = 0i64;
        if pos < end {
            // Frames already covered: before the take's start (the cursor has
            // not moved yet) they are its head; later, an overlap.
            skip = (end - pos).min(frames);
            if self.started {
                self.stats.overlapped += skip as u64;
            } else {
                self.stats.trimmed_head += skip as u64;
            }
        } else if pos > end {
            let gap = pos - end;
            self.pending
                .resize(self.pending.len() + gap as usize * CHANNELS, 0.0);
            self.stats.padded += gap as u64;
            self.started = true;
        }
        if skip < frames {
            self.pending
                .extend_from_slice(&samples[skip as usize * CHANNELS..frames as usize * CHANNELS]);
            self.started = true;
        }
    }

    /// The audio from the cursor up to master frame `limit` (exclusive), as
    /// far as it has arrived. Nothing past `limit` is handed out.
    pub fn take_until(&mut self, limit: i64) -> Vec<f32> {
        let have = (self.pending.len() / CHANNELS) as i64;
        let n = (limit - self.cursor).clamp(0, have);
        self.cursor += n;
        self.pending.drain(..n as usize * CHANNELS).collect()
    }

    /// The take's end at master frame `end`: the rest up to `end`, padded
    /// with silence where the tap fell short; anything past `end` is dropped.
    pub fn finish(mut self, end: i64) -> (Vec<f32>, AlignStats) {
        let mut out = self.take_until(end);
        if self.cursor < end {
            let short = end - self.cursor;
            out.resize(out.len() + short as usize * CHANNELS, 0.0);
            self.stats.padded_tail += short as u64;
            self.cursor = end;
        }
        self.stats.trimmed_tail += (self.pending.len() / CHANNELS) as u64;
        (out, self.stats)
    }

    /// The master frame of the next audio frame to be handed out.
    pub fn cursor(&self) -> i64 {
        self.cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` frames whose left sample is their master frame number (so every
    /// frame is identifiable) and right sample its negation.
    fn frames(from: i64, n: i64) -> Vec<f32> {
        (from..from + n)
            .flat_map(|f| [f as f32, -(f as f32)])
            .collect()
    }

    fn lefts(samples: &[f32]) -> Vec<f32> {
        samples.chunks(CHANNELS).map(|f| f[0]).collect()
    }

    #[test]
    fn audio_from_before_the_take_is_trimmed_and_the_take_starts_at_its_first_frame() {
        let mut a = TakeAudio::new(100);
        a.accept(90, &frames(90, 20));
        assert_eq!(
            lefts(&a.take_until(110)),
            (100..110).map(|f| f as f32).collect::<Vec<_>>()
        );
        let (rest, stats) = a.finish(110);
        assert!(rest.is_empty());
        assert_eq!(stats.trimmed_head, 10);
        assert_eq!(
            stats,
            AlignStats {
                trimmed_head: 10,
                ..Default::default()
            }
        );
    }

    #[test]
    fn audio_that_starts_late_is_preceded_by_silence() {
        let mut a = TakeAudio::new(100);
        a.accept(104, &frames(104, 6));
        let out = a.take_until(110);
        assert_eq!(
            lefts(&out),
            vec![0.0, 0.0, 0.0, 0.0, 104.0, 105.0, 106.0, 107.0, 108.0, 109.0]
        );
        let (_, stats) = a.finish(110);
        assert_eq!(stats.padded, 4);
    }

    #[test]
    fn nothing_past_the_latest_video_frame_is_handed_out_until_the_video_reaches_it() {
        let mut a = TakeAudio::new(0);
        a.accept(0, &frames(0, 30));
        assert_eq!(a.take_until(10).len(), 10 * CHANNELS);
        assert_eq!(a.cursor(), 10);
        assert_eq!(a.take_until(10).len(), 0, "the limit did not move");
        assert_eq!(a.take_until(25).len(), 15 * CHANNELS);
    }

    #[test]
    fn the_end_drops_what_follows_the_last_frame_and_pads_what_never_arrived() {
        let mut a = TakeAudio::new(0);
        a.accept(0, &frames(0, 12));
        let (out, stats) = a.finish(10);
        assert_eq!(lefts(&out), (0..10).map(|f| f as f32).collect::<Vec<_>>());
        assert_eq!(stats.trimmed_tail, 2);

        let mut b = TakeAudio::new(0);
        b.accept(0, &frames(0, 7));
        let (out, stats) = b.finish(10);
        assert_eq!(
            out.len(),
            10 * CHANNELS,
            "the take's audio covers its video exactly"
        );
        assert_eq!(lefts(&out)[7..], [0.0, 0.0, 0.0]);
        assert_eq!(stats.padded_tail, 3);
    }

    #[test]
    fn a_gap_inside_the_take_is_silence_and_an_overlap_is_dropped() {
        let mut a = TakeAudio::new(0);
        a.accept(0, &frames(0, 5));
        a.accept(8, &frames(8, 2));
        a.accept(9, &frames(9, 3));
        let (out, stats) = a.finish(12);
        assert_eq!(
            lefts(&out),
            vec![0.0, 1.0, 2.0, 3.0, 4.0, 0.0, 0.0, 0.0, 8.0, 9.0, 10.0, 11.0]
        );
        assert_eq!(stats.padded, 3);
        assert_eq!(stats.overlapped, 1);
    }
}
