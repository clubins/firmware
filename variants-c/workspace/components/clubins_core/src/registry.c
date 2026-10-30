//! Registry implementation — the C port of firmware-core's registry.rs.
//!
//! A fixed-capacity container: pushes fail loudly when full (a dropped
//! driver is a lost measurement stream, never a silent one), sampling
//! walks registration order and hands each fresh measurement to the sink.

#include "registry.h"

void clubins_registry_init(clubins_registry_t *registry)
{
    if (registry == NULL) {
        return;
    }
    registry->len = 0;
}

clubins_registry_status_t clubins_registry_push(clubins_registry_t *registry,
                                                const clubins_sensor_ops_t *ops, void *ctx)
{
    if (registry == NULL || ops == NULL) {
        return CLUBINS_REGISTRY_ERR_BAD_ARGUMENT;
    }
    if (registry->len >= CLUBINS_REGISTRY_MAX_SENSORS) {
        return CLUBINS_REGISTRY_ERR_FULL;
    }
    registry->sensors[registry->len].ops = ops;
    registry->sensors[registry->len].ctx = ctx;
    registry->len++;
    return CLUBINS_REGISTRY_OK;
}

size_t clubins_registry_len(const clubins_registry_t *registry)
{
    return (registry == NULL) ? 0 : registry->len;
}

bool clubins_registry_is_empty(const clubins_registry_t *registry)
{
    return clubins_registry_len(registry) == 0;
}

const clubins_sensor_t *clubins_registry_at(const clubins_registry_t *registry, size_t index)
{
    if (registry == NULL || index >= registry->len) {
        return NULL;
    }
    return &registry->sensors[index];
}

void clubins_registry_init_all(clubins_registry_t *registry)
{
    if (registry == NULL) {
        return;
    }
    for (size_t i = 0; i < registry->len; i++) {
        const clubins_sensor_t *sensor = &registry->sensors[i];
        if (sensor->ops != NULL && sensor->ops->init != NULL) {
            (void)sensor->ops->init(&registry->sensors[i]);
        }
    }
}

void clubins_registry_sample_all(clubins_registry_t *registry, clubins_measure_sink_t sink,
                                 void *sink_ctx)
{
    if (registry == NULL || sink == NULL) {
        return;
    }
    for (size_t i = 0; i < registry->len; i++) {
        clubins_sensor_t *sensor = &registry->sensors[i];
        clubins_measurement_t measurement;
        if (sensor->ops != NULL && sensor->ops->sample != NULL &&
            sensor->ops->sample(sensor, &measurement)) {
            sink(sink_ctx, &measurement);
        }
    }
}
