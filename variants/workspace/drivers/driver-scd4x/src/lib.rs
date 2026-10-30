//! SCD4x CO2 driver (the `sensor-scd4x` capability).
//!
//! Emits the co2 type — a new type with no server receiver yet, shipped per
//! the one-topic-per-type rule (spec: known gaps). The I2C front-end and the
//! conversion math land with the I2C-driver task.

#![cfg_attr(not(test), no_std)]

use firmware_core::measurement::Measurement;
use firmware_core::sensor::{Sensor, SensorError, SensorMeta};

/// SCD4x fixed I2C address (7-bit).
pub const DEFAULT_I2C_ADDRESS: u8 = 0x62;

pub const META: SensorMeta = SensorMeta {
    name: "SCD4x",
    ingest_type: "co2",
    unit: "ppm",
};

pub struct Scd4xDriver {
    i2c_address: u8,
    initialised: bool,
}

impl Scd4xDriver {
    pub fn new() -> Self {
        Self {
            i2c_address: DEFAULT_I2C_ADDRESS,
            initialised: false,
        }
    }

    pub fn i2c_address(&self) -> u8 {
        self.i2c_address
    }
}

impl Default for Scd4xDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Scd4xDriver {
    fn metadata(&self) -> &'static SensorMeta {
        &META
    }

    fn init(&mut self) -> Result<(), SensorError> {
        if self.initialised {
            return Err(SensorError::AlreadyInitialised);
        }
        self.initialised = true;
        Ok(())
    }

    fn sample(&mut self) -> Option<Measurement> {
        if !self.initialised {
            return None;
        }
        // No reading yet: register access and conversion math land with the
        // I2C driver task.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_feeds_the_new_co2_type() {
        assert_eq!(META.ingest_type, "co2");
        assert_eq!(META.unit, "ppm");
    }

    #[test]
    fn uses_the_scd4x_i2c_address() {
        assert_eq!(Scd4xDriver::new().i2c_address(), 0x62);
    }
}
