//! The registry of compiled, config-enabled sensors.

use alloc::boxed::Box;
use heapless::Vec;

use crate::measurement::Measurement;
use crate::sensor::{Sensor, SensorError};

/// Upper bound on simultaneously registered sensors: seven drivers plus
/// headroom for a second instance of a sensor type.
pub const MAX_SENSORS: usize = 8;

/// Every sensor the binary compiled in and the config enabled.
///
/// Built at boot; disabled sensors are absent, not idle, so a dead driver
/// costs neither flash nor cycles.
pub struct Registry {
    sensors: Vec<Box<dyn Sensor>, MAX_SENSORS>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub const fn new() -> Self {
        Self {
            sensors: Vec::new(),
        }
    }

    /// Register a sensor. Fails loudly when the registry is full — a driver
    /// dropped here would silently lose a measurement stream.
    pub fn push(&mut self, sensor: Box<dyn Sensor>) -> Result<(), SensorError> {
        self.sensors
            .push(sensor)
            .map_err(|_| SensorError::Fault("registry full"))
    }

    pub fn len(&self) -> usize {
        self.sensors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sensors.is_empty()
    }

    /// Sample every registered sensor, handing each fresh measurement to
    /// `sink`. Allocates nothing.
    pub fn sample_all(&mut self, mut sink: impl FnMut(Measurement)) {
        for sensor in self.sensors.iter_mut() {
            if let Some(measurement) = sensor.sample() {
                sink(measurement);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Measurement;
    use crate::sensor::SensorMeta;

    /// Stub that yields its queue once, then nothing.
    struct StubSensor {
        readings: heapless::Vec<Measurement, 4>,
    }

    impl StubSensor {
        fn new(readings: heapless::Vec<Measurement, 4>) -> Self {
            Self { readings }
        }
    }

    impl Sensor for StubSensor {
        fn metadata(&self) -> &'static SensorMeta {
            &SensorMeta {
                name: "stub",
                ingest_type: "status",
                unit: "",
            }
        }

        fn init(&mut self) -> Result<(), SensorError> {
            Ok(())
        }

        fn sample(&mut self) -> Option<Measurement> {
            if self.readings.is_empty() {
                None
            } else {
                Some(self.readings.remove(0))
            }
        }
    }

    fn one_reading() -> heapless::Vec<Measurement, 4> {
        let mut readings = heapless::Vec::new();
        readings
            .push(Measurement::Pax { occupancy_count: 2 })
            .unwrap();
        readings
    }

    #[test]
    fn sample_all_drains_every_sensor() {
        let mut registry = Registry::new();
        registry
            .push(Box::new(StubSensor::new(one_reading())))
            .unwrap();
        registry
            .push(Box::new(StubSensor::new(one_reading())))
            .unwrap();

        let mut received = 0;
        registry.sample_all(|measurement| {
            assert_eq!(measurement.ingest_type(), "pax");
            received += 1;
        });
        assert_eq!(received, 2);

        // Drained: the next pass sees nothing.
        registry.sample_all(|_| panic!("no measurement should be left"));
    }

    #[test]
    fn push_rejects_when_full() {
        let mut registry = Registry::new();
        for _ in 0..MAX_SENSORS {
            registry
                .push(Box::new(StubSensor::new(heapless::Vec::new())))
                .unwrap();
        }
        assert!(
            registry
                .push(Box::new(StubSensor::new(heapless::Vec::new())))
                .is_err()
        );
    }

    #[test]
    fn empty_registry_samples_nothing() {
        let mut registry = Registry::new();
        assert!(registry.is_empty());
        registry.sample_all(|_| panic!("nothing registered"));
    }
}
