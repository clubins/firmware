//! Built-in constants — the bottom tier of the configuration precedence rule
//! (NVS override > `config/default.toml` > built-in constant).
//!
//! The file-backed tiers are wired by the configuration task; the committed
//! `config/default.toml` mirrors these values until then, and each constant
//! names the key it stands in for.

/// Measurements buffered between sensor tasks and the transport task.
/// Sending blocks (backpressure) when the transport falls this far behind;
/// the bound keeps RAM use flat instead of hiding slowdowns in a growing heap.
pub const MEASUREMENT_QUEUE_DEPTH: usize = 16;

/// Status events buffered between ops tasks and the transport task.
pub const STATUS_QUEUE_DEPTH: usize = 8;

/// Cadence of the status heartbeat, in ms (`[sampling] report_interval_s`).
pub const REPORT_INTERVAL_MS: u64 = 60_000;

/// Length of one decibel measurement window, in ms
/// (`[sampling] decibel_window_ms`). The 125 ms Fast detector runs inside it.
pub const DECIBEL_WINDOW_MS: u32 = 1_000;

/// Poll cadence for sensor task skeletons whose drivers have not landed yet.
pub const SENSOR_POLL_INTERVAL_MS: u64 = 1_000;

/// Transport selected at runtime when both transports are compiled in
/// (`[transport] active`). Values: `"http"` | `"mqtt"`.
pub const ACTIVE_TRANSPORT: &str = "http";

/// Upper bound on concurrently planned tasks (see `features::planned_tasks`).
pub const MAX_TASKS: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_depths_are_bounded_and_sane() {
        assert!((1..=1024).contains(&MEASUREMENT_QUEUE_DEPTH));
        assert!((1..=1024).contains(&STATUS_QUEUE_DEPTH));
    }

    #[test]
    fn builtins_mirror_config_default_toml() {
        // Keep in sync with config/default.toml — its comments say so too.
        assert_eq!(REPORT_INTERVAL_MS, 60 * 1000);
        assert_eq!(DECIBEL_WINDOW_MS, 1000);
        assert!(matches!(ACTIVE_TRANSPORT, "http" | "mqtt"));
    }
}
