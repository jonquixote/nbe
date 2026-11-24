//! Master clock (SPEC §11): monotonic, STOPPED/RUNNING (HELD/SLAVE reserved).
//! `masterFrame = floor(elapsedSeconds * houseFrameRate)`.

use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockState {
    Stopped,
    Running,
}

/// The register the engine's clock finite state machine exposes.
/// In v0.3.2 only STOPPED and RUNNING are implemented.
pub struct MasterClock {
    house_rate: u32,
    state: ClockState,
    epoch: Option<Instant>,
    /// The run that last stopped: its epoch and the master frame it read at
    /// its stop (R33's run edge).
    last_run: Option<(Instant, u64)>,
}

impl MasterClock {
    pub fn new(house_rate: u32) -> Self {
        Self {
            house_rate,
            state: ClockState::Stopped,
            epoch: None,
            last_run: None,
        }
    }

    pub fn state(&self) -> ClockState {
        self.state
    }

    pub fn house_rate(&self) -> u32 {
        self.house_rate
    }

    pub fn start(&mut self) {
        if self.state == ClockState::Stopped {
            self.epoch = Some(Instant::now());
            self.state = ClockState::Running;
        }
    }

    /// Start a stopped clock so that it read `frame` at the instant `at`, as
    /// if it had been running all along: its epoch is set `frame / rate`
    /// before `at`. SPEC §5.9.4, v0.4.16 row 1: a restarted render node
    /// resumes the show's clock where the last one would be.
    ///
    /// `at` is when the frame was true (the resync's receipt), not when the
    /// clock starts. The resync loads the package first, which took about
    /// 1.8 s in the failover drill, and a clock started "at `frame`, now"
    /// came back 55 frames behind.
    ///
    /// A running clock is left alone: its own epoch is better than any
    /// estimate. An epoch that would fall before this process's monotonic
    /// origin starts the clock at `at`.
    pub fn start_at(&mut self, frame: u64, at: Instant) {
        if self.state != ClockState::Stopped {
            return;
        }
        let rate = u128::from(self.house_rate.max(1));
        let nanos = u64::try_from(u128::from(frame) * 1_000_000_000 / rate).unwrap_or(u64::MAX);
        self.epoch = Some(
            at.checked_sub(std::time::Duration::from_nanos(nanos))
                .unwrap_or(at),
        );
        self.state = ClockState::Running;
    }

    pub fn stop(&mut self) {
        if let (ClockState::Running, Some(epoch)) = (self.state, self.epoch) {
            self.last_run = self.frame_at(Instant::now()).map(|f| (epoch, f));
        }
        self.state = ClockState::Stopped;
        self.epoch = None;
    }

    /// The run that last stopped: its epoch and the master frame the clock
    /// read at its stop. The render loop counts the frames a wake past the
    /// stop never drew against it (R33's run edge). It outlives a restart; the
    /// epoch says which run it was.
    pub fn last_run(&self) -> Option<(Instant, u64)> {
        self.last_run
    }

    /// The instant master frame 0 began; None while STOPPED. A device sink
    /// measures its skew against it (device-audio design D2).
    pub fn epoch(&self) -> Option<Instant> {
        match self.state {
            ClockState::Stopped => None,
            ClockState::Running => self.epoch,
        }
    }

    /// The instant master frame `frame` begins, `epoch + frame / rate`: the
    /// inverse of [`frame`](Self::frame), rounded up to the nanosecond so the
    /// clock reads `frame` from it on. None while STOPPED. Arithmetic on the
    /// epoch alone, so the render loop can sleep to the clock's own boundary
    /// on any OS (R33).
    pub fn boundary(&self, frame: u64) -> Option<Instant> {
        let rate = u128::from(self.house_rate.max(1));
        let nanos = (u128::from(frame) * 1_000_000_000).div_ceil(rate);
        Some(self.epoch()? + std::time::Duration::from_nanos(u64::try_from(nanos).ok()?))
    }

    /// Current master frame; None while STOPPED.
    pub fn frame(&self) -> Option<u64> {
        self.frame_at(Instant::now())
    }

    /// The master frame at `now`, `floor(elapsed × rate)` in whole
    /// nanoseconds; None while STOPPED. Integer arithmetic, exact: the
    /// floating-point form read 28 at the instant frame 29 begins at 25 fps
    /// (1.16 × 25 = 28.999…), so it disagreed with [`boundary`](Self::boundary)
    /// for that nanosecond (R33's fix, 2026-10-28).
    pub fn frame_at(&self, now: Instant) -> Option<u64> {
        match self.state {
            ClockState::Stopped => None,
            ClockState::Running => {
                let elapsed = now.saturating_duration_since(self.epoch?);
                let frames = elapsed.as_nanos() * u128::from(self.house_rate) / 1_000_000_000;
                u64::try_from(frames).ok()
            }
        }
    }

    /// The master frame at `now`, signed: negative for an instant before the
    /// epoch (frame -1 is the one that ends where frame 0 begins). `floor`, as
    /// [`frame_at`](Self::frame_at), which it equals from the epoch on. None
    /// while STOPPED. The null-sink audio driver places a block due before
    /// the clock started here, before the show, where `frame_at` would read
    /// frame 0 for every such block.
    pub fn frame_at_signed(&self, now: Instant) -> Option<i64> {
        match self.state {
            ClockState::Stopped => None,
            ClockState::Running => {
                let epoch = self.epoch?;
                let nanos: i128 = if now >= epoch {
                    i128::try_from((now - epoch).as_nanos()).ok()?
                } else {
                    -i128::try_from((epoch - now).as_nanos()).ok()?
                };
                let frames = (nanos * i128::from(self.house_rate)).div_euclid(1_000_000_000);
                i64::try_from(frames).ok()
            }
        }
    }

    /// Test seam: set epoch to a known instant.
    #[cfg(test)]
    pub fn with_epoch(mut self, epoch: Instant, state: ClockState) -> Self {
        self.epoch = Some(epoch);
        self.state = state;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_clock_started_at_a_frame_reads_that_frame_and_runs_on_from_it() {
        // SPEC §5.9.4, v0.4.16 row 1: a restarted render node resumes the
        // show's clock. `start` would read 0 here: the jump the failover drill
        // measured (-170 frames).
        let mut clock = MasterClock::new(30);
        let received = Instant::now();
        clock.start_at(5_400, received);
        assert_eq!(
            clock.frame_at(received),
            Some(5_400),
            "reads the frame at the instant it named"
        );
        assert_eq!(
            clock.frame_at(received + Duration::from_secs(1)),
            Some(5_430),
            "and runs on at the house rate"
        );
        // Started late (the package load in between), it still reads where the
        // show is, not 1.8 s behind.
        let mut late = MasterClock::new(30);
        late.start_at(5_400, Instant::now() - Duration::from_millis(1_800));
        let f = late.frame().unwrap();
        assert!(
            (5_454..=5_455).contains(&f),
            "1.8 s after the frame was true: got {f}"
        );

        // A running clock keeps its own epoch.
        let epoch = clock.epoch().unwrap();
        clock.start_at(10, Instant::now());
        assert_eq!(clock.epoch(), Some(epoch), "a running clock is not moved");
    }

    #[test]
    fn the_signed_frame_reads_before_the_epoch_and_agrees_with_frame_at_after_it() {
        let epoch = Instant::now();
        let clock = MasterClock::new(30).with_epoch(epoch, ClockState::Running);
        let ms = Duration::from_millis;
        assert_eq!(clock.frame_at_signed(epoch - ms(1)), Some(-1));
        assert_eq!(clock.frame_at_signed(epoch - ms(34)), Some(-2));
        assert_eq!(clock.frame_at_signed(epoch), Some(0));
        for t in [ms(0), ms(1), ms(33), ms(34), ms(1_000), ms(123_457)] {
            assert_eq!(
                clock.frame_at_signed(epoch + t),
                clock.frame_at(epoch + t).map(|f| f as i64),
                "from the epoch on the two agree ({t:?})"
            );
        }
        assert_eq!(clock.frame_at(epoch - ms(1)), Some(0), "frame_at saturates");
    }

    #[test]
    fn a_boundary_is_the_epoch_plus_its_frames_rounded_up_to_the_nanosecond() {
        let epoch = Instant::now() - Duration::from_secs(10);
        let c = MasterClock::new(30).with_epoch(epoch, ClockState::Running);
        assert_eq!(c.boundary(0), Some(epoch));
        assert_eq!(
            c.boundary(1),
            Some(epoch + Duration::from_nanos(33_333_334))
        );
        assert_eq!(c.boundary(30), Some(epoch + Duration::from_secs(1)));
        assert_eq!(c.boundary(300), Some(epoch + Duration::from_secs(10)));
        // The frame the clock reads now began at or before now, and the next
        // one begins after it.
        let now = Instant::now();
        let f = c.frame_at(now).unwrap();
        assert!(c.boundary(f).unwrap() <= now && now < c.boundary(f + 1).unwrap());
        assert_eq!(
            MasterClock::new(30).boundary(5),
            None,
            "STOPPED has no boundaries"
        );
    }

    #[test]
    fn the_clock_names_each_frame_from_its_boundary_on_and_never_before() {
        // The render loop sleeps to `boundary(f)` and draws what the clock
        // names (R33): the two must agree at every boundary, or a wake there
        // would name the frame already drawn. Ten hours at each rate.
        let epoch = Instant::now();
        for rate in [24, 25, 30, 50, 60] {
            let c = MasterClock::new(rate).with_epoch(epoch, ClockState::Running);
            for f in 1..(10 * 3600 * u64::from(rate)) {
                let b = c.boundary(f).unwrap();
                assert_eq!(
                    c.frame_at(b),
                    Some(f),
                    "rate {rate}: frame {f} at its boundary"
                );
                assert_eq!(
                    c.frame_at(b - Duration::from_nanos(1)),
                    Some(f - 1),
                    "rate {rate}: frame {f} a nanosecond early"
                );
            }
        }
    }

    #[test]
    fn stopped_clock_never_advances() {
        let c = MasterClock::new(30);
        assert_eq!(c.frame(), None);
        assert_eq!(c.state(), ClockState::Stopped);
    }

    #[test]
    fn frame_math_is_floor_of_elapsed_times_rate() {
        let now = Instant::now();
        let c =
            MasterClock::new(30).with_epoch(now - Duration::from_millis(1000), ClockState::Running);
        // At exactly 1.000 s elapsed at 30 fps: frame 30 (0..29 ticked).
        assert_eq!(c.frame_at(now), Some(30));
        // A nanosecond short of it, still frame 29.
        assert_eq!(c.frame_at(now - Duration::from_nanos(1)), Some(29));
    }

    #[test]
    fn start_then_stop_returns_to_stopped() {
        let mut c = MasterClock::new(30);
        c.start();
        assert_eq!(c.state(), ClockState::Running);
        assert!(c.frame().is_some());
        c.stop();
        assert_eq!(c.state(), ClockState::Stopped);
        assert_eq!(c.frame(), None);
    }
}
