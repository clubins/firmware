//! Stub: GPIO PIR occupancy front-end (pin from config).
//!
//! Compiled in by `feature = "sensor-pax"`; the real driver ships in a
//! downstream PR (docs/plan/build-dag.md) and emits `pax`
//! (occupancy_count). PIR detections are reported honestly — this is a
//! motion front-end, not a head-count metric.

/// Module marker for the presence test.
pub const MODULE: &str = "sensors::pir";
