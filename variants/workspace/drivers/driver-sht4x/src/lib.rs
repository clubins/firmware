//! SHT4x temperature/humidity driver (the `sensor-sht4x` capability).
//!
//! Alternative temperature/humidity source to the BME280; when both are
//! compiled in, config picks per device. Feeds the weather ingest type. The
//! I2C front-end and conversion math land with the I2C-driver task.

#![cfg_attr(not(test), no_std)]

use firmware_core::measurement::Measurement;
use firmware_core::sensor::{Sensor, SensorError, SensorMeta};

/// SHT4x default I2C address (ADDR pin low).
pub const DEFAULT_I2C_ADDRESS: u8 = 0x44;

pub const META: SensorMeta = SensorMeta {
    name: "SHT4x",
    ingest_type: "weather",
    unit: "°C/%",
};

pub struct Sht4xDriver {
    i2c_address: u8,
    initialised: bool,
}

impl Sht4xDriver {
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

impl Default for Sht4xDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Sht4xDriver {
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
    fn metadata_feeds_the_weather_ingest_type() {
        assert_eq!(META.ingest_type, "weather");
        assert_eq!(META.name, "SHT4x");
    }

    #[test]
    fn defaults_to_the_sht4x_i2c_address() {
        let sensor = Sht4xDriver::new();
        assert_eq!(sensor.i2c_address(), 0x44);
    }
}
