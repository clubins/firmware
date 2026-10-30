//! Status heartbeat stub — gated by the `ops-status` flag (default on;
//! both ingest contracts expect it).
//!
//! HTTP contract: subsystem/result/message fields (server-infra/ingest/
//! service/status.go); MQTT v2 contract: retained Status on connect
//! (api/mqtt/topics.md). Heartbeat cadence and lifecycle assembly arrive
//! with the ops-status PR; the stub tracks beats.

/// Ingest type for status reports (one-topic-per-type rule).
pub const STATUS_TYPE: &str = "status";

#[derive(Debug)]
pub struct StatusHeartbeat {
    /// Heartbeats emitted since boot.
    pub beats: u32,
}

impl StatusHeartbeat {
    pub const fn new() -> Self {
        StatusHeartbeat { beats: 0 }
    }

    /// One heartbeat; returns the running beat count.
    pub fn beat(&mut self) -> u32 {
        self.beats += 1;
        self.beats
    }
}

impl Default for StatusHeartbeat {
    fn default() -> Self {
        Self::new()
    }
}

/// Firmware version reported in every status message.
pub fn firmware_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_count_up() {
        let mut s = StatusHeartbeat::new();
        assert_eq!(s.beat(), 1);
        assert_eq!(s.beat(), 2);
        assert_eq!(s.beats, 2);
    }

    #[test]
    fn firmware_version_is_the_crate_version() {
        assert_eq!(firmware_version(), env!("CARGO_PKG_VERSION"));
        assert!(!firmware_version().is_empty());
    }

    #[test]
    fn status_type_is_the_ingest_type() {
        assert_eq!(STATUS_TYPE, "status");
    }
}
