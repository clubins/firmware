//! BME280 driver stub — gated by the `sensor-bme280` flag.
//!
//! I2C temperature + humidity + pressure; emits the `weather` ingest type
//! (temperature/humidity/pressure fields exist server-side today; the
//! precipitation/rain/wind fields are virtual Open-Meteo values and are
//! never emitted from hardware — spec §Sensor Expansion). The I2C front-end
//! arrives with the driver PRs.

use crate::boundary::Measurement;
use crate::core1::{SensorDriver, SensorMeta};

pub static META: SensorMeta = SensorMeta {
    name: "bme280",
    ingest_type: "weather",
};

#[derive(Debug)]
pub struct Bme280Stub {
    /// Completed readings since boot (observable for tests).
    pub readings: u32,
}

impl Bme280Stub {
    pub const fn new() -> Self {
        Bme280Stub { readings: 0 }
    }
}

impl Default for Bme280Stub {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorDriver for Bme280Stub {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn poll(&mut self) -> Option<Measurement> {
        self.readings = self.readings.wrapping_add(1);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_maps_bme280_onto_the_weather_type() {
        assert_eq!(META.name, "bme280");
        assert_eq!(META.ingest_type, "weather");
    }

    #[test]
    fn scaffold_poll_reports_no_measurement() {
        let mut b = Bme280Stub::new();
        assert_eq!(b.poll(), None);
        assert_eq!(b.readings, 1);
    }
}
