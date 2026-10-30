//! Status heartbeats (the `ops-status` capability).
//!
//! Both ingest contracts require status: the HTTP path posts to
//! `/v1/sensors/status`, the MQTT path publishes the retained status
//! topic. Heartbeats are always compiled into the binary when any
//! transport is present; this crate provides the report builders and the
//! cadence policy.

#![cfg_attr(not(test), no_std)]

use firmware_core::measurement::{StatusReport, StatusResult};
use heapless::String;

/// A subsystem tag for reports.
///
/// The server stores the subsystem as a free string, but the firmware
/// treats it as a closed vocabulary so a typo cannot masquerade as a
/// subsystem on the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsystem {
    Boot,
    Sampling,
    Publish,
    Ota,
    Provisioning,
    Clock,
}

impl Subsystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Subsystem::Boot => "boot",
            Subsystem::Sampling => "sampling",
            Subsystem::Publish => "publish",
            Subsystem::Ota => "ota",
            Subsystem::Provisioning => "provisioning",
            Subsystem::Clock => "clock",
        }
    }
}

/// Build a report for a subsystem. Truncates `message` to the report
/// buffer rather than failing: a status line is a diagnostic, and losing
/// its tail on a mis-sized buffer must not take down the report itself.
pub fn report(subsystem: Subsystem, result: StatusResult, message: &str) -> StatusReport {
    StatusReport {
        subsystem: String::try_from(subsystem.as_str()).expect("subsystem fits"),
        result,
        message: String::try_from(message).unwrap_or_default(),
    }
}

/// The boot report: result `Success`, message = the compiled feature list.
///
/// When a device shows up on the dashboard with surprising behaviour, the
/// first question is "what was it built with" — so the boot heartbeat
/// answers it.
pub fn boot_report(features: &[&str]) -> StatusReport {
    let mut message = String::<256>::new();
    for (index, feature) in features.iter().enumerate() {
        if index > 0 && message.push(',').is_err() {
            break;
        }
        if message.push_str(feature).is_err() {
            break;
        }
    }
    report(Subsystem::Boot, StatusResult::Success, message.as_str())
}

/// Cadence policy: a heartbeat is due when at least `interval_s` seconds
/// have passed since the last one. A clock that jumped backwards (SNTP
/// re-sync, RTC change) counts as "due immediately" — a non-monotonic clock
/// must never suppress a heartbeat.
pub fn heartbeat_due(last_sent_s: u64, now_s: u64, interval_s: u64) -> bool {
    if now_s < last_sent_s {
        return true;
    }
    now_s - last_sent_s >= interval_s
}

#[cfg(test)]
mod tests {
    use super::*;
    use firmware_core::measurement::Measurement;

    #[test]
    fn subsystem_vocabulary_is_the_documented_set() {
        assert_eq!(Subsystem::Boot.as_str(), "boot");
        assert_eq!(Subsystem::Sampling.as_str(), "sampling");
        assert_eq!(Subsystem::Publish.as_str(), "publish");
        assert_eq!(Subsystem::Ota.as_str(), "ota");
        assert_eq!(Subsystem::Provisioning.as_str(), "provisioning");
        assert_eq!(Subsystem::Clock.as_str(), "clock");
    }

    #[test]
    fn boot_report_lists_the_compiled_features() {
        let report = boot_report(&["sensor-decibel", "transport-http", "ops-status"]);
        assert_eq!(report.subsystem.as_str(), "boot");
        assert_eq!(report.result, StatusResult::Success);
        assert_eq!(
            report.message.as_str(),
            "sensor-decibel,transport-http,ops-status"
        );
    }

    #[test]
    fn boot_report_truncates_instead_of_panicking_on_a_huge_list() {
        let features: Vec<&str> =
            core::iter::repeat_n("flag-with-a-long-name", 64).collect();
        let report = boot_report(&features);
        assert_eq!(report.result, StatusResult::Success);
        // Everything that fits is there — 256/22 = 11 full entries.
        assert!(report.message.starts_with("flag-with-a-long-name,"));
        assert!(report.message.len() < 256);
    }

    #[test]
    fn heartbeats_are_due_on_interval_and_saturate_clock_stalls() {
        assert!(heartbeat_due(0, 60, 60));
        assert!(!heartbeat_due(0, 59, 60));
        assert!(heartbeat_due(0, 120, 60));
        // Clock went backwards: saturating to 0 means "due now", never a
        // skipped heartbeat.
        assert!(heartbeat_due(120, 60, 60));
    }

    #[test]
    fn report_carries_the_server_status_fields() {
        let report = report(
            Subsystem::Publish,
            StatusResult::Failed,
            "broker unreachable",
        );
        assert_eq!(report.subsystem.as_str(), "publish");
        assert_eq!(report.result, StatusResult::Failed);
        assert_eq!(report.message.as_str(), "broker unreachable");
        // The measurement is well-formed for the status ingest type.
        assert_eq!(Measurement::from(report).ingest_type(), "status");
    }
}
