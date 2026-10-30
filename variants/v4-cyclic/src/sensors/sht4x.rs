//! SHT4x over I2C — temperature and humidity.
//!
//! Alternative to the BME280's temp/hum half; both may be compiled and the
//! runtime config picks per-device. No pressure channel — its weather
//! measurement leaves `pressure` unset.
//!
//! Stub: the SHT4x command protocol (measure-high-repeatability 0xFD) and CRC
//! check land with the driver task.

use crate::bus::Bus;
use crate::sensor::{IngestType, Sample, Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "sht4x",
    ingest_type: IngestType::Weather,
    report_period_ticks: 30_000,
};

pub struct Sht4x {
    /// I2C address — 0x44 (default) or 0x45/0x46 per variant.
    _address: u8,
}

impl Sht4x {
    pub const DEFAULT_ADDRESS: u8 = 0x44;

    pub const fn new() -> Self {
        Self {
            _address: Self::DEFAULT_ADDRESS,
        }
    }
}

impl Default for Sht4x {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Sht4x {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn init(&mut self, _bus: &mut Bus) -> Result<(), SensorError> {
        // Stub: serial-number read (0x89) lands with the driver.
        Ok(())
    }

    fn sample(&mut self, _bus: &mut Bus) -> Result<Option<Sample>, SensorError> {
        Ok(None) // Stub: conversion + CRC land with the driver task.
    }
}
