//! Decibel driver stub — gated by the `sensor-decibel` flag.
//!
//! Driver contract (spec §Sensor Expansion + implementation_notes/decibel.md
//! in the source monorepo): SPK0641HT4H-1 PDM mic on I2S0 at 48 kHz,
//! decimation to PCM, A/C weighting, LAeq/LCeq with Fast extrema, overload
//! detection, per-unit calibration. The DSP chain belongs to this core-1
//! partition; the driver PR replaces the stub body behind the same
//! [`SensorDriver`] impl.

use crate::boundary::{Ack, Control, Measurement};
use crate::core1::{SensorDriver, SensorMeta};

pub static META: SensorMeta = SensorMeta {
    name: "decibel",
    ingest_type: "decibel",
};

#[derive(Debug)]
pub struct DecibelStub {
    /// Per-unit calibration offset (dB), settable from core 0 at runtime;
    /// the NVS-loaded value arrives with the config PR (decibel.md).
    pub calibration_offset_db: f32,
    /// Overload detection threshold (dB) — decibel.md.
    pub overload_threshold_db: f32,
    /// Completed windows since boot (observable for tests).
    pub windows: u32,
}

impl DecibelStub {
    pub const fn new() -> Self {
        DecibelStub {
            calibration_offset_db: 0.0,
            overload_threshold_db: 120.0,
            windows: 0,
        }
    }
}

impl Default for DecibelStub {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorDriver for DecibelStub {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn poll(&mut self) -> Option<Measurement> {
        // I2S PDM front-end + DSP arrive with the driver PRs; the scaffold
        // reports no measurement on device.
        self.windows = self.windows.wrapping_add(1);
        None
    }

    fn on_control(&mut self, cmd: Control) -> Option<Ack> {
        match cmd {
            Control::SetCalibrationDb(db) => {
                self.calibration_offset_db = db;
                Some(Ack::CalibrationSet)
            }
            Control::SetOverloadThresholdDb(db) => {
                self.overload_threshold_db = db;
                Some(Ack::OverloadThresholdSet)
            }
            // Partition-level commands (park/resume/shutdown) are the
            // system's, not the driver's.
            Control::ParkSampling | Control::ResumeSampling | Control::Shutdown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_matches_the_one_topic_per_type_contract() {
        assert_eq!(META.name, "decibel");
        assert_eq!(META.ingest_type, "decibel");
    }

    #[test]
    fn calibration_commands_apply_and_ack() {
        let mut d = DecibelStub::new();
        assert_eq!(
            d.on_control(Control::SetCalibrationDb(2.5)),
            Some(Ack::CalibrationSet)
        );
        assert_eq!(d.calibration_offset_db, 2.5);
        assert_eq!(
            d.on_control(Control::SetOverloadThresholdDb(110.0)),
            Some(Ack::OverloadThresholdSet)
        );
        assert_eq!(d.overload_threshold_db, 110.0);
    }

    #[test]
    fn partition_level_commands_are_not_driver_concerns() {
        let mut d = DecibelStub::new();
        assert_eq!(d.on_control(Control::ParkSampling), None);
        assert_eq!(d.on_control(Control::ResumeSampling), None);
        assert_eq!(d.on_control(Control::Shutdown), None);
    }

    #[test]
    fn scaffold_poll_reports_no_measurement() {
        let mut d = DecibelStub::new();
        assert_eq!(d.poll(), None);
        assert_eq!(d.windows, 1);
    }
}
