//! SPK0641HT4H-1 PDM microphone driver — the flagship sensor.
//!
//! Chain per `hardware-firmware/implementation_notes/decibel.md` (the design
//! this driver must implement, not an interpretation): I2S0 PDM capture at
//! 48 kHz → decimation to PCM → DC removal → A- and C-weighting filters →
//! 125 ms Fast windows → LAeq/LCeq with window extrema → overload detection →
//! per-unit calibration offset.
//!
//! Stub: the I2S front-end and the DSP chain land with the decibel tasks; the
//! DSP module this flag compiles (`crate::dsp`) already carries the window
//! types and the seam the samples flow through.

use crate::bus::Bus;
use crate::dsp::{DecibelWindow, DspChain};
use crate::sensor::{IngestType, Sample, Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "decibel",
    ingest_type: IngestType::Decibel,
    report_period_ticks: 60, // 1 s windows at a 1 kHz grid
};

pub struct Decibel {
    /// Feed-through for the DSP chain once the I2S front-end lands.
    dsp: DspChain,
}

impl Decibel {
    pub const fn new() -> Self {
        Self {
            dsp: DspChain::new(48_000),
        }
    }

    /// The completed window, if the DSP has one ready — the comms phase
    /// turns this into a `decibel` measurement (dba/dbc).
    pub fn window(&self) -> Option<DecibelWindow> {
        self.dsp.completed_window()
    }
}

impl Default for Decibel {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Decibel {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn init(&mut self, _bus: &mut Bus) -> Result<(), SensorError> {
        // Stub: I2S0 PDM RX setup (48 kHz, 64× decimation) lands with the
        // real front-end; the calibration offset arrives from NVS at boot.
        Ok(())
    }

    fn sample(&mut self, _bus: &mut Bus) -> Result<Option<Sample>, SensorError> {
        // Stub: no PCM stream yet, so no window. Never fabricates a level.
        Ok(self.window().map(|w| Sample::Decibel {
            dba: w.laeq_a,
            dbc: w.lceq_c,
            la_fmin: w.la_fmin,
            la_fmax: w.la_fmax,
            lc_fmax: w.lc_fmax,
            window_ms: w.window_ms,
            overload: w.overload,
        }))
    }
}
