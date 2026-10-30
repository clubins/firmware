//! BME280 environmental sensor (the `sensor-bme280` capability) — the
//! C port of the driver-bme280 crate's scaffold. Temperature, humidity
//! and pressure readings land with the I2C bus task (0x76 is the config
//! convention for the address); the measurement model's env type is
//! already wire-shaped.

#pragma once

#include <stdbool.h>

#include "sensor.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Static metadata (the port of driver-bme280::META). */
extern const clubins_sensor_meta_t clubins_bme280_meta;

/** Driver instance state (the port of the bme280 scaffold's driver
 * struct). */
typedef struct {
    bool initialised;
} clubins_bme280_driver_t;

/** Construct the driver. */
void clubins_bme280_driver_new(clubins_bme280_driver_t *driver);

/** The sensor ops — pass to clubins_registry_push with the driver as ctx. */
extern const clubins_sensor_ops_t clubins_bme280_sensor_ops;

#ifdef __cplusplus
}
#endif
