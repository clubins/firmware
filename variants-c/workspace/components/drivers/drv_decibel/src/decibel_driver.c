//! Decibel driver implementation — the scaffold of the driver-decibel
//! crate: lifecycle and metadata now, the DSP chain and I2S front-end
//! with the decibel tasks.

#include "decibel_driver.h"

const clubins_sensor_meta_t clubins_decibel_meta = {
    .name = "SPK0641HT4H-1",
    .ingest_type = "decibel",
    .unit = "dB SPL",
};

void clubins_decibel_driver_new(clubins_decibel_driver_t *driver)
{
    if (driver == NULL) {
        return;
    }
    driver->initialised = false;
}

static clubins_sensor_status_t decibel_init(clubins_sensor_t *self)
{
    clubins_decibel_driver_t *driver = self->ctx;
    if (driver->initialised) {
        return CLUBINS_SENSOR_ERR_ALREADY_INITIALISED;
    }
    driver->initialised = true;
    return CLUBINS_SENSOR_OK;
}

static bool decibel_sample(clubins_sensor_t *self, clubins_measurement_t *out)
{
    (void)out;
    const clubins_decibel_driver_t *driver = self->ctx;
    if (!driver->initialised) {
        return false;
    }
    /* No measurement window has finished: the PDM front-end and the DSP
     * chain (weighting, Leq accumulation, Fast extrema, overload) are the
     * decibel tasks' deliverable and sample nothing until they land. */
    return false;
}

const clubins_sensor_ops_t clubins_decibel_sensor_ops = {
    .meta = &clubins_decibel_meta,
    .init = decibel_init,
    .sample = decibel_sample,
};
