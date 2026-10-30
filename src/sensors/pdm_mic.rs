//! Stub: SPK0641HT4H-1 PDM microphone front-end (I2S0 PDM @ 48 kHz) with the
//! A/C-weighted decibel DSP from `decibel.md` in the source monorepo.
//!
//! Compiled in by `feature = "sensor-decibel"`; the real driver ships in a
//! downstream PR (docs/plan/build-dag.md) and emits `decibel` (dba, dbc),
//! with per-unit calibration from NVS.

/// Module marker for the presence test.
pub const MODULE: &str = "sensors::pdm_mic";
