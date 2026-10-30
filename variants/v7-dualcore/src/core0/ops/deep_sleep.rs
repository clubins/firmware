//! Deep-sleep duty-cycling stub — gated by the `deep-sleep` flag.
//!
//! V7 shape: the coordinator lives on core 0 (power management is
//! latency-tolerant) and parks/resumes the sensing core through the control
//! mailbox — never by touching core-1 state directly. The mailbox hookup
//! lands with the cross-core primitive; this module pins the cadence math.

/// What the coordinator wants done this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    None,
    /// Park the sensing partition (start of a sleep window).
    Park,
    /// Resume sampling (end of a sleep window).
    Resume,
}

#[derive(Debug)]
pub struct SleepCoordinator {
    /// Awake ticks before a park is requested.
    pub park_after_ticks: u32,
    /// Sleep-window length in ticks.
    pub park_duration_ticks: u32,
    awake_ticks: u32,
    parked_ticks: u32,
    /// True while a park is in flight or being served.
    pub parking: bool,
}

impl SleepCoordinator {
    pub const fn new(park_after_ticks: u32, park_duration_ticks: u32) -> Self {
        SleepCoordinator {
            park_after_ticks,
            park_duration_ticks,
            awake_ticks: 0,
            parked_ticks: 0,
            parking: false,
        }
    }

    /// Pure cadence machine: one decision per tick.
    pub fn advance(&mut self) -> Decision {
        if !self.parking {
            self.awake_ticks += 1;
            if self.awake_ticks >= self.park_after_ticks {
                self.parking = true;
                self.parked_ticks = 0;
                return Decision::Park;
            }
        } else {
            self.parked_ticks += 1;
            if self.parked_ticks >= self.park_duration_ticks {
                self.parking = false;
                self.awake_ticks = 0;
                return Decision::Resume;
            }
        }
        Decision::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_alternates_park_and_resume() {
        let mut c = SleepCoordinator::new(2, 2);
        assert_eq!(c.advance(), Decision::None);
        assert_eq!(c.advance(), Decision::Park);
        assert_eq!(c.advance(), Decision::None);
        assert_eq!(c.advance(), Decision::Resume);
        assert_eq!(c.advance(), Decision::None);
        assert_eq!(c.advance(), Decision::Park); // cycle repeats
    }

    #[test]
    fn park_window_is_never_zero_ticks() {
        let mut c = SleepCoordinator::new(1, 3);
        assert_eq!(c.advance(), Decision::Park);
        for _ in 0..2 {
            assert_eq!(c.advance(), Decision::None);
        }
        assert_eq!(c.advance(), Decision::Resume);
    }
}
