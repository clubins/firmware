//! Stub: SPK0641HT4H-1 PDM microphone front-end (I2S0 PDM @ 48 kHz) with
//! the A/C-weighted decibel DSP from the source monorepo's decibel.md.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_SENSOR_DECIBEL; the real driver ships
//! in a downstream C task and emits `decibel` (dba, dbc), with per-unit
//! calibration from NVS.

#include "stubkit.h"

void clubins_stub_pdm_mic_init(void)
{
    /* Real I2S PDM bring-up lands with the decibel driver task. */
}

void clubins_stub_pdm_mic_tick(void)
{
    /* Real sampling/windowing lands with the decibel driver task. */
}
