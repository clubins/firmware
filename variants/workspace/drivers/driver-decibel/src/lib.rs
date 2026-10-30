//! PDM microphone front-end + decibel DSP pipeline (the `sensor-decibel`
//! capability).
//!
//! Implemented exactly per the source monorepo's
//! `hardware-firmware/implementation_notes/decibel.md`: SPK0641HT4H-1 on I2S0
//! PDM at 48 kHz, decimation to PCM, A- and C-weighting, LAeq/LCeq with
//! Fast-weighted extrema, overload detection, per-unit calibration from NVS.
//! The pure DSP chain and the I2S front-end land with the decibel tasks; this
//! scaffold pins the driver's surface, its metadata and its config coupling.

#![cfg_attr(not(test), no_std)]

use firmware_core::config::DecibelConfig;
use firmware_core::measurement::Measurement;
use firmware_core::sensor::{Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "SPK0641HT4H-1",
    ingest_type: "decibel",
    unit: "dB SPL",
};

pub struct DecibelSensor {
    overload_threshold_db: f32,
    calibration_offset_db: f32,
    initialised: bool,
}

impl DecibelSensor {
    pub fn new(config: &DecibelConfig) -> Self {
        Self {
            overload_threshold_db: config.overload_threshold_db,
            calibration_offset_db: config.calibration_offset_db,
            initialised: false,
        }
    }

    pub fn overload_threshold_db(&self) -> f32 {
        self.overload_threshold_db
    }

    /// Per-unit value; the NVS override replaces the config default at boot.
    pub fn calibration_offset_db(&self) -> f32 {
        self.calibration_offset_db
    }
}

impl Sensor for DecibelSensor {
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
        // No measurement window has finished: the PDM front-end and the DSP
        // chain (weighting, Leq accumulation, Fast extrema, overload) are the
        // decibel tasks' deliverable and sample None until they land.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> DecibelConfig {
        DecibelConfig {
            overload_threshold_db: 118.5,
            calibration_offset_db: -2.5,
        }
    }

    #[test]
    fn metadata_feeds_the_decibel_ingest_type() {
        assert_eq!(META.ingest_type, "decibel");
        assert_eq!(META.name, "SPK0641HT4H-1");
        assert_eq!(META.unit, "dB SPL");
    }

    #[test]
    fn carries_the_config_calibration_and_overload_values() {
        let sensor = DecibelSensor::new(&config());
        assert_eq!(sensor.calibration_offset_db(), -2.5);
        assert_eq!(sensor.overload_threshold_db(), 118.5);
    }

    #[test]
    fn sample_is_none_before_and_after_init_until_the_dsp_lands() {
        let config = config();
        let mut sensor = DecibelSensor::new(&config);
        assert!(sensor.sample().is_none());
        sensor.init().unwrap();
        assert!(sensor.sample().is_none());
    }

    #[test]
    fn init_twice_is_an_error() {
        let config = config();
        let mut sensor = DecibelSensor::new(&config);
        sensor.init().unwrap();
        assert_eq!(sensor.init(), Err(SensorError::AlreadyInitialised));
    }
}
