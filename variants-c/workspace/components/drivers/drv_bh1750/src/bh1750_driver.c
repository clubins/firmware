//! BH1750 driver implementation — the scaffold of the driver-bh1750
//! crate: lifecycle and metadata now, the I2C front-end with the bus
//! task.

#include "bh1750_driver.h"

const clubins_sensor_meta_t clubins_bh1750_meta = {
    .name = "BH1750",
    .ingest_type = "lux",
    .unit = "lx",
};

void clubins_bh1750_driver_new(clubins_bh1750_driver_t *driver)
{
    if (driver == NULL) {
        return;
    }
    driver->initialised = false;
}

static clubins_sensor_status_t bh1750_init(clubins_sensor_t *self)
{
    clubins_bh1750_driver_t *driver = self->ctx;
    if (driver->initialised) {
        return CLUBINS_SENSOR_ERR_ALREADY_INITIALISED;
    }
    driver->initialised = true;
    return CLUBINS_SENSOR_OK;
}

static bool bh1750_sample(clubins_sensor_t *self, clubins_measurement_t *out)
{
    (void)out;
    const clubins_bh1750_driver_t *driver = self->ctx;
    if (!driver->initialised) {
        return false;
    }
    /* No readings yet: the hardware front-end is the bus task's deliverable
     * and samples nothing until it lands. */
    return false;
}

const clubins_sensor_ops_t clubins_bh1750_sensor_ops = {
    .meta = &clubins_bh1750_meta,
    .init = bh1750_init,
    .sample = bh1750_sample,
};
