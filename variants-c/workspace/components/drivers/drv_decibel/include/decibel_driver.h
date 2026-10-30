//! PDM microphone front-end + decibel DSP pipeline (the `sensor-decibel`
//! capability) — the C port of the driver-decibel crate's scaffold.
//!
//! Will implement exactly the source monorepo's
//! `hardware-firmware/implementation_notes/decibel.md`: SPK0641HT4H-1 on
//! I2S0 PDM at 48 kHz, decimation to PCM, A- and C-weighting, LAeq/LCeq
//! with Fast-weighted extrema, overload detection, per-unit calibration
//! from NVS. The pure DSP chain and the I2S front-end land with the
//! decibel tasks; the config coupling (overload threshold, per-unit
//! calibration offset) arrives with them reading the config pool. This
//! scaffold pins the driver's surface, metadata and lifecycle.

#pragma once

#include <stdbool.h>

#include "sensor.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Static metadata (the port of driver_decibel::META). */
extern const clubins_sensor_meta_t clubins_decibel_meta;

/** Driver instance state (the port of the DecibelSensor struct). */
typedef struct {
    bool initialised;
} clubins_decibel_driver_t;

/** Construct the driver (the port of DecibelSensor::new). */
void clubins_decibel_driver_new(clubins_decibel_driver_t *driver);

/** The sensor ops — pass to clubins_registry_push with the driver as ctx. */
extern const clubins_sensor_ops_t clubins_decibel_sensor_ops;

#ifdef __cplusplus
}
#endif
