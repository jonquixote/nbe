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

    /// Current master frame; None while STOPPED.
    pub fn frame(&self) -> Option<u64> {
        match self.state {
            ClockState::Stopped => None,
            ClockState::Running => {
                let elapsed = self.epoch?.elapsed();
                Some((elapsed.as_secs_f64() * self.house_rate as f64).floor() as u64)
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
