//! Plantower PMS5003 particulate sensor (the `sensor-pms5003`
//! capability) — the C port of the driver-pms5003 crate's scaffold.
//! The UART front-end and particulate readings land with the bus task.

#pragma once

#include <stdbool.h>

#include "sensor.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Static metadata (the port of driver-pms5003::META). */
extern const clubins_sensor_meta_t clubins_pms5003_meta;

/** Driver instance state (the port of the pms5003 scaffold's driver
 * struct). */
typedef struct {
    bool initialised;
} clubins_pms5003_driver_t;

/** Construct the driver. */
void clubins_pms5003_driver_new(clubins_pms5003_driver_t *driver);

/** The sensor ops — pass to clubins_registry_push with the driver as ctx. */
extern const clubins_sensor_ops_t clubins_pms5003_sensor_ops;

#ifdef __cplusplus
}
#endif
