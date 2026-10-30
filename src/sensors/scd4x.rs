//! Stub: SCD4x over I2C — CO2 (ppm).
//!
//! Compiled in by `feature = "sensor-scd4x"`; the real driver ships in a
//! downstream PR (docs/plan/build-dag.md) and emits `co2` — a new
//! measurement type following the one-topic-per-type rule (no server
//! receiver yet; that is a deliberate, tracked gap).

/// Module marker for the presence test.
pub const MODULE: &str = "sensors::scd4x";
