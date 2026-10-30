//! SHT4x driver stub — gated by the `sensor-sht4x` flag.
//!
//! I2C temperature + humidity; alternative to the BME280's temp/hum pair.
//! When both are compiled in, runtime config picks the per-device source
//! (spec §Sensor Expansion). The I2C front-end arrives with the driver PRs.

use crate::boundary::Measurement;
use crate::core1::{SensorDriver, SensorMeta};

pub static META: SensorMeta = SensorMeta {
    name: "sht4x",
    ingest_type: "weather",
};

#[derive(Debug)]
pub struct Sht4xStub {
    /// Completed readings since boot (observable for tests).
    pub readings: u32,
}

impl Sht4xStub {
    pub const fn new() -> Self {
        Sht4xStub { readings: 0 }
    }
}

impl Default for Sht4xStub {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorDriver for Sht4xStub {
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
    fn meta_maps_sht4x_onto_the_weather_type() {
        assert_eq!(META.name, "sht4x");
        assert_eq!(META.ingest_type, "weather");
    }

    #[test]
    fn scaffold_poll_reports_no_measurement() {
        let mut s = Sht4xStub::new();
        assert_eq!(s.poll(), None);
        assert_eq!(s.readings, 1);
    }
}
