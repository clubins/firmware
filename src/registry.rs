//! Feature-flag resolution: which drivers are compiled into this build.
//!
//! The runtime registry — owned driver objects, built at boot from config —
//! lands with the driver PRs. The scaffold owns the compile-time half that
//! transports and config build on: the authoritative mapping from cargo
//! feature selection to the sensor set a build carries.

use crate::sensor::{SensorKind, SensorMeta};

/// Metadata for every sensor driver compiled in via its `sensor-*` flag, in
/// [`SensorKind::ALL`] order. Each arm is a compile-time constant (via
/// `cfg!`), so a build without a flag carries none of its driver code.
pub fn enabled_sensor_metas() -> impl Iterator<Item = SensorMeta> {
    [
        cfg!(feature = "sensor-decibel").then_some(SensorMeta::new(SensorKind::Decibel)),
        cfg!(feature = "sensor-pax").then_some(SensorMeta::new(SensorKind::Pax)),
        cfg!(feature = "sensor-bme280").then_some(SensorMeta::new(SensorKind::Bme280)),
        cfg!(feature = "sensor-sht4x").then_some(SensorMeta::new(SensorKind::Sht4x)),
        cfg!(feature = "sensor-scd4x").then_some(SensorMeta::new(SensorKind::Scd4x)),
        cfg!(feature = "sensor-pms5003").then_some(SensorMeta::new(SensorKind::Pms5003)),
        cfg!(feature = "sensor-bh1750").then_some(SensorMeta::new(SensorKind::Bh1750)),
    ]
    .into_iter()
    .flatten()
}
