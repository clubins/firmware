//! Measurement model implementation — the name tables and the equality
//! oracle. Every string here is contract bytes: the type names are the HTTP
//! path suffixes and MQTT topic suffixes; the result names are status.go's
//! knownResults verbatim.

#include "measurement.h"

#include <string.h>

const char *const clubins_measurement_type_names[CLUBINS_MEAS_TYPE_COUNT] = {
    "decibel", "pax", "weather", "co2", "pm25", "light", "status",
};

const char *const clubins_status_result_names[CLUBINS_STATUS_RESULT_COUNT] = {
    "started", "stopped", "success", "degraded", "failed",
};

const char *clubins_measurement_type_name(clubins_meas_type_t type)
{
    /* size_t cast: out-of-range or negative ids wrap into the >= check. */
    if ((size_t)type >= CLUBINS_MEAS_TYPE_COUNT) {
        return NULL;
    }
    return clubins_measurement_type_names[type];
}

const char *clubins_measurement_ingest_type(const clubins_measurement_t *measurement)
{
    if (measurement == NULL) {
        return NULL;
    }
    return clubins_measurement_type_name(measurement->type);
}

const char *clubins_status_result_name(clubins_status_result_t result)
{
    if ((size_t)result >= CLUBINS_STATUS_RESULT_COUNT) {
        return NULL;
    }
    return clubins_status_result_names[result];
}

bool clubins_status_result_parse(const char *name, clubins_status_result_t *out)
{
    if (name == NULL || out == NULL) {
        return false;
    }
    for (size_t i = 0; i < CLUBINS_STATUS_RESULT_COUNT; i++) {
        if (strcmp(name, clubins_status_result_names[i]) == 0) {
            *out = (clubins_status_result_t)i;
            return true;
        }
    }
    return false;
}

bool clubins_measurement_equal(const clubins_measurement_t *a, const clubins_measurement_t *b)
{
    if (a == NULL || b == NULL || a->type != b->type) {
        return false;
    }
    switch (a->type) {
    case CLUBINS_MEAS_STATUS:
    {
        /* Strings first: NULL renders as "" on the wire, so two NULLs and
         * two empty strings are the same status. */
        const char *a_sub = (a->as.status.subsystem != NULL) ? a->as.status.subsystem : "";
        const char *b_sub = (b->as.status.subsystem != NULL) ? b->as.status.subsystem : "";
        const char *a_msg = (a->as.status.message != NULL) ? a->as.status.message : "";
        const char *b_msg = (b->as.status.message != NULL) ? b->as.status.message : "";
        return a->as.status.result == b->as.status.result && strcmp(a_sub, b_sub) == 0 &&
               strcmp(a_msg, b_msg) == 0;
    }
    case CLUBINS_MEAS_DECIBEL:
        return a->as.decibel.dba == b->as.decibel.dba && a->as.decibel.dbc == b->as.decibel.dbc;
    case CLUBINS_MEAS_PAX:
        return a->as.pax.occupancy_count == b->as.pax.occupancy_count;
    case CLUBINS_MEAS_WEATHER:
        /* Field by field: struct padding bytes are unspecified, so a
         * byte-compare can fail two equal measurements built on different
         * stacks. */
        return a->as.weather.has_temperature == b->as.weather.has_temperature &&
               a->as.weather.temperature_c == b->as.weather.temperature_c &&
               a->as.weather.has_humidity == b->as.weather.has_humidity &&
               a->as.weather.humidity_pct == b->as.weather.humidity_pct &&
               a->as.weather.has_pressure == b->as.weather.has_pressure &&
               a->as.weather.pressure_hpa == b->as.weather.pressure_hpa;
    case CLUBINS_MEAS_CO2:
        return a->as.co2.ppm == b->as.co2.ppm;
    case CLUBINS_MEAS_PM25:
        return a->as.pm25.ug_m3 == b->as.pm25.ug_m3;
    case CLUBINS_MEAS_LIGHT:
        return a->as.light.lux == b->as.light.lux;
    case CLUBINS_MEAS_TYPE_COUNT:
        return true;
    default:
        return false;
    }
}
