//! Capability registry — the public surface of the flag framework's
//! compile-time half (the C port of V1's sensor.rs + registry.rs).
//!
//! - clubins_cap_name / clubins_sensor_ingest_type: static identity tables,
//!   addressable for every capability regardless of selection.
//! - clubins_cap_enabled: the per-arm compile-time resolution of the
//!   Kconfig selection (V1's `cfg!(feature = ...)` arms).
//! - clubins_registry_modules: the compiled-in module set, in stable
//!   registry order — disabled = absent, never a stub with a dead flag.
//! - clubins_registry_init_all / tick_all: the lifecycle the device main
//!   loop (FreeRTOS timer) and the host tests drive.

#pragma once

#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Every public capability leaf, in stable registry order (spec §Flag
 * Matrix). Plumbing flags are intentionally not listed here. */
typedef enum {
    CLUBINS_CAP_SENSOR_DECIBEL = 0,
    CLUBINS_CAP_SENSOR_PAX,
    CLUBINS_CAP_SENSOR_BME280,
    CLUBINS_CAP_SENSOR_SHT4X,
    CLUBINS_CAP_SENSOR_SCD4X,
    CLUBINS_CAP_SENSOR_PMS5003,
    CLUBINS_CAP_SENSOR_BH1750,
    CLUBINS_CAP_TRANSPORT_HTTP,
    CLUBINS_CAP_TRANSPORT_MQTT,
    CLUBINS_CAP_TRANSPORT_MTLS,
    CLUBINS_CAP_OTA,
    CLUBINS_CAP_PROVISIONING,
    CLUBINS_CAP_DEEP_SLEEP,
    CLUBINS_CAP_LOGGING_DEFMT,
    CLUBINS_CAP_OPS_STATUS,
    CLUBINS_CAP_COUNT
} clubins_cap_id_t;

/** Sentinel cap id for "not a real capability" (registry table padding). */
#define CLUBINS_CAP_NONE ((clubins_cap_id_t)CLUBINS_CAP_COUNT)

/** Static description of one compiled-in capability module: identity, wire
 * type, and the (currently empty) lifecycle hooks the main loop drives. */
typedef struct {
    clubins_cap_id_t cap;
    /** Module marker, the port of V1's `MODULE` consts, e.g.
     * "sensors::pdm_mic". */
    const char *name;
    /** Ingest measurement type on the wire; NULL for non-sensor modules. */
    const char *ingest_type;
    void (*init)(void);
    void (*tick)(void);
} clubins_module_t;

/** Sensor-only view of the registry (the port of V1's SensorMeta). */
typedef struct {
    clubins_cap_id_t cap;
    /** Ingest measurement type (one topic per type on the wire). */
    const char *ingest_type;
} clubins_sensor_meta_t;

/** Upper bounds for caller-provided tables: at most one module per cap,
 * and the sensor families are the seven sensor-* leaves. */
#define CLUBINS_CAP_MAX_MODULES ((size_t)CLUBINS_CAP_COUNT)
#define CLUBINS_CAP_MAX_SENSORS ((size_t)7)

/** Module marker for a capability id; valid for all ids, always. */
const char *clubins_cap_name(clubins_cap_id_t cap);

/** The ingest measurement type a sensor family feeds; NULL when the id is
 * not a sensor family. Both contracts keep one topic per measurement
 * type; `weather` is shared by the temperature/humidity/pressure drivers
 * (spec §Sensor Expansion). */
const char *clubins_sensor_ingest_type(clubins_cap_id_t cap);

/** Compile-time resolution of the Kconfig selection for one capability. */
bool clubins_cap_enabled(clubins_cap_id_t cap);

/** The compiled-in modules, in stable registry order. Writes at most `max`
 * pointers into `out` and returns the number written. */
size_t clubins_registry_modules(const clubins_module_t **out, size_t max);

/** The compiled-in sensor metas, in stable registry order (the port of
 * registry::enabled_sensor_metas). Writes at most `max` entries into `out`
 * and returns the number written. */
size_t clubins_enabled_sensor_metas(clubins_sensor_meta_t *out, size_t max);

/** Run init on every compiled-in module. Called once at boot. */
void clubins_registry_init_all(void);

/** Run tick on every compiled-in module. Called from the periodic FreeRTOS
 * timer callback (device) or the test harness (host). */
void clubins_registry_tick_all(void);

#ifdef __cplusplus
}
#endif
