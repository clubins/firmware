//! Stub: PMS5003 over UART — PM2.5 particulates (µg/m³).
//!
//! Compiled in by `feature = "sensor-pms5003"`; the real driver ships in a
//! downstream PR (docs/plan/build-dag.md) and emits `pm25` — a new
//! measurement type following the one-topic-per-type rule (no server
//! receiver yet; that is a deliberate, tracked gap).

/// Module marker for the presence test.
pub const MODULE: &str = "sensors::pms5003";
