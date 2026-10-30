//! BH1750 ambient-light driver (the `sensor-bh1750` capability).
//!
//! Emits the light type — a new type with no server receiver yet, shipped per
//! the one-topic-per-type rule (spec: known gaps). The I2C front-end lands
//! with the I2C-driver task.

#![cfg_attr(not(test), no_std)]

use firmware_core::measurement::Measurement;
use firmware_core::sensor::{Sensor, SensorError, SensorMeta};

/// BH1750 default I2C address (ADDR pin low).
pub const DEFAULT_I2C_ADDRESS: u8 = 0x23;

pub const META: SensorMeta = SensorMeta {
    name: "BH1750",
    ingest_type: "light",
    unit: "lx",
};

pub struct Bh1750Driver {
    i2c_address: u8,
    initialised: bool,
}

impl Bh1750Driver {
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

impl Default for Bh1750Driver {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Bh1750Driver {
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
        // No reading yet: register access lands with the I2C driver task.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_feeds_the_new_light_type() {
        assert_eq!(META.ingest_type, "light");
        assert_eq!(META.unit, "lx");
    }

    #[test]
    fn uses_the_bh1750_i2c_address() {
        assert_eq!(Bh1750Driver::new().i2c_address(), 0x23);
    }
}
