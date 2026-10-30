//! BME280 temperature/humidity/pressure driver (the `sensor-bme280`
//! capability).
//!
//! Feeds the weather ingest type; precipitation, rain probability and wind
//! speed have no firmware source (virtual Open-Meteo fields server-side).
//! The I2C front-end and the compensation-math conversion land with the bus
//! and I2C-driver tasks; this scaffold pins the surface and addressing.

#![cfg_attr(not(test), no_std)]

use firmware_core::config::Bme280Config;
use firmware_core::measurement::Measurement;
use firmware_core::sensor::{Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "BME280",
    ingest_type: "weather",
    unit: "°C/%/hPa",
};

pub struct Bme280Driver {
    i2c_address: u8,
    initialised: bool,
}

impl Bme280Driver {
    pub fn new(config: &Bme280Config) -> Self {
        Self {
            i2c_address: config.i2c_address,
            initialised: false,
        }
    }

    pub fn i2c_address(&self) -> u8 {
        self.i2c_address
    }
}

impl Sensor for Bme280Driver {
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
        // No reading yet: register access and compensation math land with the
        // I2C driver task; sample None until then.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_feeds_the_weather_ingest_type() {
        assert_eq!(META.ingest_type, "weather");
        assert_eq!(META.name, "BME280");
    }

    #[test]
    fn uses_the_configured_i2c_address() {
        let config = Bme280Config {
            enabled: true,
            i2c_address: 0x76,
        };
        let sensor = Bme280Driver::new(&config);
        assert_eq!(sensor.i2c_address(), 0x76);
    }
}
