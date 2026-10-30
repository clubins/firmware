//! Stub: BME280 over I2C — temperature, humidity, pressure.
//!
//! Compiled in by `feature = "sensor-bme280"`; the real driver ships in a
//! downstream PR (docs/plan/build-dag.md) and emits `weather`
//! (temperature, humidity, pressure).

/// Module marker for the presence test.
pub const MODULE: &str = "sensors::bme280";
