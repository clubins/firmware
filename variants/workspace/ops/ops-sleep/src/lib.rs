//! Deep-sleep duty cycling (the `deep-sleep` capability): sample, publish,
//! sleep — trading measurement continuity for battery.
//!
//! The esp-hal sleep entry and wakeup-cause handling land with the deep
//! sleep task; this scaffold pins the duty-cycle schedule math.

#![cfg_attr(not(test), no_std)]

/// A duty-cycle schedule derived from config: how long to stay awake and
/// how long to sleep. Pure math — testable on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DutyCycle {
    /// Seconds awake per cycle (sampling + one publish attempt).
    pub awake_s: u32,
    /// Seconds in deep sleep per cycle.
    pub sleep_s: u32,
}

/// An error for a schedule where awake + sleep does not fit the intended
/// duty share, or non-positive intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSchedule;

impl DutyCycle {
    /// Build a schedule that wakes every `interval_s` seconds and stays
    /// awake for the given slice of it (at least one second, at most the
    /// whole interval — a zero-sleep schedule means "never sleep").
    pub fn from_interval(interval_s: u32, awake_s: u32) -> Result<Self, InvalidSchedule> {
        if interval_s == 0 || awake_s == 0 || awake_s >= interval_s {
            return Err(InvalidSchedule);
        }
        Ok(Self {
            awake_s,
            sleep_s: interval_s - awake_s,
        })
    }

    /// Awake share of the cycle in percent (integer, floor).
    pub fn duty_percent(&self) -> u32 {
        (self.awake_s * 100) / (self.awake_s + self.sleep_s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_interval_into_awake_and_sleep() {
        let cycle = DutyCycle::from_interval(60, 5).unwrap();
        assert_eq!(cycle.awake_s, 5);
        assert_eq!(cycle.sleep_s, 55);
        assert_eq!(cycle.duty_percent(), 8); // 5/60 ≈ 8.3%
    }

    #[test]
    fn rejects_degenerate_schedules() {
        assert_eq!(DutyCycle::from_interval(0, 1), Err(InvalidSchedule));
        assert_eq!(DutyCycle::from_interval(60, 0), Err(InvalidSchedule));
        // Awake >= interval leaves nothing to sleep: config error.
        assert_eq!(DutyCycle::from_interval(60, 60), Err(InvalidSchedule));
        assert_eq!(DutyCycle::from_interval(60, 61), Err(InvalidSchedule));
    }
}
