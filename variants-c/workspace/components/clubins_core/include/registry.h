//! The registry of compiled, composed sensors — the C port of
//! firmware-core's registry.rs.
//!
//! Every sensor the binary compiled in and the composition registered.
//! Built at boot from the composition root (main/app_compose.c); disabled
//! sensors are absent, not idle, so a dead driver costs neither flash nor
//! cycles — a capability left out of the Kconfig selection has no
//! component in the link at all.

#pragma once

#include <stdbool.h>
#include <stddef.h>

#include "sensor.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Upper bound on simultaneously registered sensors: seven drivers plus
 * headroom for a second instance of a sensor type. */
#define CLUBINS_REGISTRY_MAX_SENSORS 8

/** Registry outcomes (the port of Registry::push's SensorError). */
typedef enum {
    CLUBINS_REGISTRY_OK = 0,
    /** Push past capacity — a driver dropped here would silently lose a
     * measurement stream, so this fails loudly (V2: Fault("registry full")). */
    CLUBINS_REGISTRY_ERR_FULL = -1,
    /** NULL registry or NULL ops. */
    CLUBINS_REGISTRY_ERR_BAD_ARGUMENT = -2,
} clubins_registry_status_t;

/** Sink for fresh measurements (V2's `sink: impl FnMut(Measurement)`). */
typedef void (*clubins_measure_sink_t)(void *sink_ctx, const clubins_measurement_t *measurement);

/** Fixed-capacity sensor registry — no heap, no allocation. */
typedef struct {
    clubins_sensor_t sensors[CLUBINS_REGISTRY_MAX_SENSORS];
    size_t len;
} clubins_registry_t;

/** Empty registry, ready for pushes. */
void clubins_registry_init(clubins_registry_t *registry);

/** Register a sensor. Fails loudly when the registry is full — a driver
 * dropped here would silently lose a measurement stream. */
clubins_registry_status_t clubins_registry_push(clubins_registry_t *registry,
                                                const clubins_sensor_ops_t *ops, void *ctx);

/** How many sensors are registered. */
size_t clubins_registry_len(const clubins_registry_t *registry);

/** True when nothing is registered. */
bool clubins_registry_is_empty(const clubins_registry_t *registry);

/** Registered sensor by index, in registration order; NULL out of range. */
const clubins_sensor_t *clubins_registry_at(const clubins_registry_t *registry, size_t index);

/** Bring up every registered sensor once, in registration order (the
 * "called once at bring-up, before any sample" half of the sensor
 * contract). Real drivers surface faults through their sample path and
 * status reporting when those land. */
void clubins_registry_init_all(clubins_registry_t *registry);

/** Sample every registered sensor, handing each fresh measurement to
 * `sink`. Allocates nothing. */
void clubins_registry_sample_all(clubins_registry_t *registry, clubins_measure_sink_t sink,
                                 void *sink_ctx);

#ifdef __cplusplus
}
#endif
