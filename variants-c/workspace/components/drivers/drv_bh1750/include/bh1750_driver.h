//! ROHM BH1750 ambient-light sensor (the `sensor-lux` capability) —
//! the C port of the driver-bh1750 crate's scaffold. The I2C front-end
//! and lux readings land with the bus task.

#pragma once

#include <stdbool.h>

#include "sensor.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Static metadata (the port of driver-bh1750::META). */
extern const clubins_sensor_meta_t clubins_bh1750_meta;

/** Driver instance state (the port of the bh1750 scaffold's driver
 * struct). */
typedef struct {
    bool initialised;
} clubins_bh1750_driver_t;

/** Construct the driver. */
void clubins_bh1750_driver_new(clubins_bh1750_driver_t *driver);

/** The sensor ops — pass to clubins_registry_push with the driver as ctx. */
extern const clubins_sensor_ops_t clubins_bh1750_sensor_ops;

#ifdef __cplusplus
}
#endif
