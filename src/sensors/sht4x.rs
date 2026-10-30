//! Stub: SHT4x over I2C — temperature, humidity.
//!
//! Compiled in by `feature = "sensor-sht4x"`; the real driver ships in a
//! downstream PR (docs/plan/build-dag.md) and emits `weather`
//! (temperature, humidity). Alternative to BME280's temp/hum; when both are
//! compiled in, config picks per-device.

/// Module marker for the presence test.
pub const MODULE: &str = "sensors::sht4x";
