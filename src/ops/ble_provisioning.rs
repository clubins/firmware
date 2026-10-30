//! Stub: BLE-based Wi-Fi credential provisioning.
//!
//! Compiled in by `feature = "provisioning"`; the real implementation ships
//! in a downstream PR (docs/plan/build-dag.md). Credentials land in NVS
//! only — never in the committed config.

/// Module marker for the presence test.
pub const MODULE: &str = "ops::ble_provisioning";
