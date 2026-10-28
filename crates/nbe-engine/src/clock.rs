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
}

impl MasterClock {
    pub fn new(house_rate: u32) -> Self {
        Self {
            house_rate,
            state: ClockState::Stopped,
            epoch: None,
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

    pub fn stop(&mut self) {
        self.state = ClockState::Stopped;
        self.epoch = None;
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
    fn frame_at(&self, now: Instant) -> Option<u64> {
        match self.state {
            ClockState::Stopped => None,
            ClockState::Running => {
                let elapsed = now.saturating_duration_since(self.epoch?);
                let frames = elapsed.as_nanos() * u128::from(self.house_rate) / 1_000_000_000;
                u64::try_from(frames).ok()
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
        let f = c.frame().unwrap();
        let now = Instant::now();
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
        let f = c.frame().unwrap();
        assert!(f == 30 || f == 31, "frame {} not in [30,31]", f);
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
