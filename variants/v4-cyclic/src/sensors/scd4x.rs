//! SCD4x over I2C — CO₂ concentration.
//!
//! Emits the new `co2` ingest type per the one-topic-per-type rule; the
//! server grows a receiver for it later (tracked gap in the build spec).
//!
//! Stub: start/stop periodic measurement (0x21B6/0x0104), read (0xEC05) and
//! the SENSIRION word CRC land with the driver task.

use crate::bus::Bus;
use crate::sensor::{IngestType, Sample, Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "scd4x",
    ingest_type: IngestType::Co2,
    report_period_ticks: 30_000,
};

pub struct Scd4x {
    /// I2C address — 0x62, fixed for the family.
    _address: u8,
}

impl Scd4x {
    pub const ADDRESS: u8 = 0x62;

    pub const fn new() -> Self {
        Self {
            _address: Self::ADDRESS,
        }
    }
}

impl Default for Scd4x {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Scd4x {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn init(&mut self, _bus: &mut Bus) -> Result<(), SensorError> {
        // Stub: periodic-measurement start lands with the driver.
        Ok(())
    }

    fn sample(&mut self, _bus: &mut Bus) -> Result<Option<Sample>, SensorError> {
        Ok(None) // Stub: CO₂ read + data-ready check land with the driver task.
    }
}
