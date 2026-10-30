//! Decibel DSP chain — the pure-logic half of the acoustic pipeline.
//!
//! Specified end-to-end in `implementation_notes/decibel.md`: DC removal, then
//! A- and C-weighting (IEC 61672-1), energy accumulation over 125 ms Fast
//! windows, LAeq/LCeq conversion, Fast extrema tracked continuously across
//! windows, overload detection against the configurable threshold, and a
//! per-unit calibration offset applied to every level.
//!
//! Stub: the types and the seam exist; the filter math lands with the decibel
//! tasks and is host-tested there against synthetic sines at reference levels.

/// One completed measurement window — field-for-field with
/// `api/mqtt/v2/sensor/decibel.proto` (MeasurementMeta, MeasurementA,
/// MeasurementC). All levels in dB SPL.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecibelWindow {
    /// A-weighted equivalent continuous level (LAeq).
    pub laeq_a: f32,
    /// C-weighted equivalent continuous level (LCeq).
    pub lceq_c: f32,
    /// Fast-weighted extrema within the window.
    pub la_fmin: f32,
    pub la_fmax: f32,
    pub lc_fmax: f32,
    /// Length of the measurement window.
    pub window_ms: u32,
    /// The input reached the top of the sensor's range inside this window —
    /// every level here may be wrong, usually too low. Never silently hidden.
    pub overload: bool,
}

/// The sample → window pipeline. Fixed state, no allocation: weighting filter
/// coefficients and accumulators sized at construction.
pub struct DspChain {
    sample_rate_hz: u32,
}

impl DspChain {
    pub const fn new(sample_rate_hz: u32) -> Self {
        Self { sample_rate_hz }
    }

    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Completed window since the last call, if any.
    pub fn completed_window(&self) -> Option<DecibelWindow> {
        None // Stub: filter math lands with the decibel DSP task.
    }
}
