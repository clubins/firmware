//! Duty-cycled sampling between deep sleeps (`deep-sleep` feature).
//!
//! The esp_sleep bring-up lands in the driver PR; the budget rule is fixed
//! here because the engine's cadences are what the sleep budget must honour:
//! the device stays awake only long enough to sample and drain.

/// Awake time a duty cycle reserves for sampling and draining (ms).
pub const MIN_AWAKE_MS: u64 = 100;

/// Sleep budget for one duty cycle: the reporting interval minus the time
/// the device must stay awake to use it, floored at [`MIN_AWAKE_MS`].
pub fn sleep_duration_ms(report_interval_ms: u64, awake_ms: u64) -> u64 {
    report_interval_ms
        .saturating_sub(awake_ms)
        .max(MIN_AWAKE_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_fills_the_interval_outside_the_awake_window() {
        assert_eq!(sleep_duration_ms(60_000, 500), 59_500);
    }

    #[test]
    fn tight_budgets_keep_the_minimum_awake_floor() {
        // An interval barely above the awake window never sleeps to zero.
        assert_eq!(sleep_duration_ms(150, 100), MIN_AWAKE_MS);
        // Degenerate inputs stay bounded, never underflow.
        assert_eq!(sleep_duration_ms(50, 100), MIN_AWAKE_MS);
    }
}
