//! PMS5003 particulate-matter driver (the `sensor-pms5003` capability).
//!
//! Emits the pm25 type — a new type with no server receiver yet, shipped per
//! the one-topic-per-type rule (spec: known gaps). The UART front-end and the
//! frame parse/checksum land with the bus task.

#![cfg_attr(not(test), no_std)]

use firmware_core::measurement::Measurement;
use firmware_core::sensor::{Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "PMS5003",
    ingest_type: "pm25",
    unit: "µg/m³",
};

pub struct Pms5003Driver {
    initialised: bool,
}

impl Pms5003Driver {
    pub fn new() -> Self {
        Self { initialised: false }
    }
}

impl Default for Pms5003Driver {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Pms5003Driver {
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
        // No reading yet: the UART frame parser (start bytes, length, frame
        // checksum) lands with the bus task.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_feeds_the_new_pm25_type() {
        assert_eq!(META.ingest_type, "pm25");
        assert_eq!(META.unit, "µg/m³");
    }
}
