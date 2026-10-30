//! Ops capabilities — status heartbeats, OTA, provisioning, power, logging.
//!
//! `ops-status` is a leaf flag but on by default: both the HTTP and MQTT
//! contracts expect status reports. The hardware watchdog is not a flag at
//! all — it runs unconditionally on target as a safety property nobody opts
//! out of.

use crate::measurement::{Message, StatusResult, Subsystem};

/// One status report, shaped for both contracts (`status.go` fields; the
/// proto `Status` carries the connection state).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusEvent {
    pub subsystem: Subsystem,
    pub result: StatusResult,
    pub message: Message,
}

impl StatusEvent {
    /// Boot lifecycle opener — `result = started` on the firmware subsystem.
    pub fn boot() -> Self {
        let mut message = [0u8; 64];
        message[..4].copy_from_slice(b"boot");
        Self {
            subsystem: Subsystem::Firmware,
            result: StatusResult::Started,
            message: Message(message),
        }
    }
}

/// Boot lifecycle tracker — emits `started` once, then periodic heartbeats.
pub struct BootLifecycle {
    started: bool,
}

impl BootLifecycle {
    pub const fn new() -> Self {
        Self { started: false }
    }

    /// The next status to emit, if any — the ops phase drains this.
    pub fn next_event(&mut self) -> Option<StatusEvent> {
        if self.started {
            None
        } else {
            self.started = true;
            Some(StatusEvent::boot())
        }
    }
}

impl Default for BootLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Flagged ops stubs — each documents exactly what lands with its task.
// ---------------------------------------------------------------------------

/// OTA via the esp-ota partition scheme (`ota` flag).
///
/// Stub: partition table, image verification, and the rollback path land
/// with the OTA task.
#[cfg(feature = "ota")]
pub mod ota {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct OtaState {
        /// Image staged and verified but not yet switched to.
        pub staged: bool,
    }
}

/// BLE Wi-Fi credential provisioning (`provisioning` flag).
///
/// Stub: the BLE GATT service and NVS write path land with the provisioning
/// task; credentials go to NVS only.
#[cfg(feature = "provisioning")]
pub mod provisioning {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct ProvisioningState {
        /// A credential set arrived over BLE and landed in NVS.
        pub credentials_written: bool,
    }
}

/// Duty-cycled sampling between deep sleeps (`deep-sleep` flag).
///
/// Stub: the esp-hal deep-sleep entry, RTC timer wake source, and the
/// sleep/wake state handoff land with the power task. Trades measurement
/// continuity for power — a config decision, not a default.
#[cfg(feature = "deep-sleep")]
pub mod power {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SleepState {
        /// Wake cause consumed by the first sensor phase after boot.
        pub timer_wake: bool,
    }
}

/// defmt logging transport (`logging-defmt` flag).
///
/// Stub: the defmt-rtt/UART wiring lands with the diagnostics task. Compile
/// errors if both this and the default log backend are active — the backends
/// are exclusive (the `log`/`defmt` bridge is a documented esp-hal pattern).
#[cfg(feature = "logging-defmt")]
pub mod logging {
    // defmt macros re-exported once the transport task wires the sink.
}
