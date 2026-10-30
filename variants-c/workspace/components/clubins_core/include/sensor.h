//! The sensor contract every driver component implements — the C port of
//! firmware-core's sensor.rs.
//!
//! A driver is a vtable plus private state: the registry holds every
//! composed driver behind one sensor handle (V2's `Box<dyn Sensor>`) with
//! no heap — instances are static/caller-owned and registered by pointer.
//! Buses are owned by the concrete driver; the ops stay the shared shape.
//!
//! Platform-agnostic: no IDF headers — the device build and the host
//! CTest harness compile these exact bytes.

#pragma once

#include <stdbool.h>

#include "measurement.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Static description of a sensor: what it is, what it emits, in what
 * unit (the port of SensorMeta). All three point at static literals. */
typedef struct {
    /** Hardware identity, e.g. "SPK0641HT4H-1" or "BME280". */
    const char *name;
    /** The ingest type this sensor feeds (`Measurement::ingest_type`). */
    const char *ingest_type;
    /** Measurement unit, e.g. "dB SPL", "lx", "µg/m³". */
    const char *unit;
} clubins_sensor_meta_t;

/** A driver lifecycle outcome or fault (the port of SensorError). */
typedef enum {
    CLUBINS_SENSOR_OK = 0,
    /** The sensor did not answer or answered nonsense. */
    CLUBINS_SENSOR_ERR_FAULT = -1,
    /** The sensor was already initialised. */
    CLUBINS_SENSOR_ERR_ALREADY_INITIALISED = -2,
    /** Sampled before a successful init. */
    CLUBINS_SENSOR_ERR_NOT_INITIALISED = -3,
} clubins_sensor_status_t;

/** One registered sensor: ops plus the driver's private state. */
typedef struct clubins_sensor {
    const struct clubins_sensor_ops *ops;
    /** Driver-private state, owned by the driver component. */
    void *ctx;
} clubins_sensor_t;

/** The sensor ops (the port of the Sensor trait). */
typedef struct clubins_sensor_ops {
    /** Static per-driver-class metadata; never NULL. */
    const clubins_sensor_meta_t *meta;
    /** Bring the sensor up. Called once at bring-up, before any sample. */
    clubins_sensor_status_t (*init)(clubins_sensor_t *self);
    /** Take one reading; false when nothing new is due (no motion, no
     * finished measurement window, ...). */
    bool (*sample)(clubins_sensor_t *self, clubins_measurement_t *out);
} clubins_sensor_ops_t;

#ifdef __cplusplus
}
#endif
