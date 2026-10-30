//! BH1750 over I2C — ambient light.
//!
//! Emits the new `light` ingest type per the one-topic-per-type rule.
//!
//! Stub: one-time high-res measurement command (0x20) and the lux conversion
//! land with the driver task.

use crate::bus::Bus;
use crate::sensor::{IngestType, Sample, Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "bh1750",
    ingest_type: IngestType::Light,
    report_period_ticks: 30_000,
};

pub struct Bh1750 {
    /// I2C address — 0x23 (ADDR low) or 0x5C (ADDR high).
    _address: u8,
}

impl Bh1750 {
    pub const DEFAULT_ADDRESS: u8 = 0x23;

    pub const fn new() -> Self {
        Self {
            _address: Self::DEFAULT_ADDRESS,
        }
    }
}

impl Default for Bh1750 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Bh1750 {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn init(&mut self, _bus: &mut Bus) -> Result<(), SensorError> {
        // Stub: measurement command + wait land with the driver.
        Ok(())
    }

    fn sample(&mut self, _bus: &mut Bus) -> Result<Option<Sample>, SensorError> {
        Ok(None) // Stub: lux conversion lands with the driver task.
    }
}
