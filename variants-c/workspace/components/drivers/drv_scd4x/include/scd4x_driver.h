//! Sensirion SCD4x CO2 sensor (the `sensor-scd4x` capability) — the C
//! port of the driver-scd4x crate's scaffold. The I2C front-end and
//! CO2 readings land with the bus task.

#pragma once

#include <stdbool.h>

#include "sensor.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Static metadata (the port of driver-scd4x::META). */
extern const clubins_sensor_meta_t clubins_scd4x_meta;

/** Driver instance state (the port of the scd4x scaffold's driver
 * struct). */
typedef struct {
    bool initialised;
} clubins_scd4x_driver_t;

/** Construct the driver. */
void clubins_scd4x_driver_new(clubins_scd4x_driver_t *driver);

/** The sensor ops — pass to clubins_registry_push with the driver as ctx. */
extern const clubins_sensor_ops_t clubins_scd4x_sensor_ops;

#ifdef __cplusplus
}
#endif
