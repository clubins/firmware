//! Stub: BH1750 over I2C — ambient light (lx).
//!
//! Compiled in by `feature = "sensor-bh1750"`; the real driver ships in a
//! downstream PR (docs/plan/build-dag.md) and emits `light` — a new
//! measurement type following the one-topic-per-type rule (no server
//! receiver yet; that is a deliberate, tracked gap).

/// Module marker for the presence test.
pub const MODULE: &str = "sensors::bh1750";
