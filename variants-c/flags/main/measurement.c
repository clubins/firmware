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
    if (a->type == CLUBINS_MEAS_STATUS) {
        /* Strings first: NULL renders as "" on the wire, so two NULLs and
         * two empty strings are the same status. */
        const char *a_sub = (a->as.status.subsystem != NULL) ? a->as.status.subsystem : "";
        const char *b_sub = (b->as.status.subsystem != NULL) ? b->as.status.subsystem : "";
        const char *a_msg = (a->as.status.message != NULL) ? a->as.status.message : "";
        const char *b_msg = (b->as.status.message != NULL) ? b->as.status.message : "";
        return a->as.status.result == b->as.status.result && strcmp(a_sub, b_sub) == 0 &&
               strcmp(a_msg, b_msg) == 0;
    }
    /* The union members are all plain-old-data and disjoint per type, so a
     * byte compare over the union's storage is exact for every type. */
    return memcmp(&a->as, &b->as, sizeof(a->as)) == 0;
}
