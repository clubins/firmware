//! PIR occupancy front-end (the `sensor-pax` capability).
//!
//! No PIR hardware is documented in the source repo, so the driver reports
//! PIR detections honestly: a motion-event count, not a head-count metric.
//! The GPIO interrupt front-end lands with the bus task; `record_motion` is
//! the hook its interrupt handler will call.

#![cfg_attr(not(test), no_std)]

use firmware_core::measurement::Measurement;
use firmware_core::sensor::{Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "PIR motion front-end",
    ingest_type: "pax",
    unit: "detections",
};

pub struct PirSensor {
    gpio_pin: u8,
    motions_since_sample: u32,
    initialised: bool,
}

impl PirSensor {
    pub fn new(gpio_pin: u8) -> Self {
        Self {
            gpio_pin,
            motions_since_sample: 0,
            initialised: false,
        }
    }

    pub fn gpio_pin(&self) -> u8 {
        self.gpio_pin
    }

    /// Interrupt-side hook: one PIR trigger.
    pub fn record_motion(&mut self) {
        self.motions_since_sample = self.motions_since_sample.saturating_add(1);
    }
}

impl Sensor for PirSensor {
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
        if !self.initialised || self.motions_since_sample == 0 {
            return None;
        }
        let count = core::mem::take(&mut self.motions_since_sample);
        Some(Measurement::Pax {
            occupancy_count: count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_feeds_the_pax_ingest_type() {
        assert_eq!(META.ingest_type, "pax");
        assert_eq!(META.unit, "detections");
    }

    #[test]
    fn counts_motions_between_samples() {
        let mut sensor = PirSensor::new(4);
        sensor.init().unwrap();

        assert!(sensor.sample().is_none(), "no motion yet");
        sensor.record_motion();
        sensor.record_motion();
        assert_eq!(
            sensor.sample(),
            Some(Measurement::Pax { occupancy_count: 2 })
        );
        assert!(sensor.sample().is_none(), "counter drains on sample");
    }

    #[test]
    fn samples_nothing_before_init() {
        let mut sensor = PirSensor::new(4);
        sensor.record_motion();
        assert!(sensor.sample().is_none());
    }
}
