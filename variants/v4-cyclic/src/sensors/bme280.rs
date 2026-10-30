//! BME280 over I2C — temperature, humidity, pressure.
//!
//! Contract: emits `weather` (temperature, humidity, pressure). The server's
//! precipitation, rain_probability and wind_speed fields stay unset — those
//! are virtual Open-Meteo fields the device never reports.
//!
//! Stub: register map (chip ID 0x60, config, forced-mode reads) and the
//! Bosch compensation math land with the driver task.

use crate::bus::Bus;
use crate::sensor::{IngestType, Sample, Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "bme280",
    ingest_type: IngestType::Weather,
    report_period_ticks: 30_000, // 30 s weather cadence
};

pub struct Bme280 {
    /// I2C address — 0x76 or 0x77 per the SDO pin; config picks per-device.
    _address: u8,
}

impl Bme280 {
    pub const DEFAULT_ADDRESS: u8 = 0x76;

    pub const fn new() -> Self {
        Self {
            _address: Self::DEFAULT_ADDRESS,
        }
    }
}

impl Default for Bme280 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Bme280 {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn init(&mut self, _bus: &mut Bus) -> Result<(), SensorError> {
        // Stub: chip-ID check + trim-parameter read land with the driver.
        Ok(())
    }

    fn sample(&mut self, _bus: &mut Bus) -> Result<Option<Sample>, SensorError> {
        Ok(None) // Stub: raw read + compensation land with the driver task.
    }
}
