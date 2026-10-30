//! SHT4x driver implementation — the scaffold of the driver-sht4x
//! crate: lifecycle and metadata now, the I2C front-end with the bus
//! task.

#include "sht4x_driver.h"

const clubins_sensor_meta_t clubins_sht4x_meta = {
    .name = "SHT4x",
    .ingest_type = "sht4x",
    .unit = "°C, %RH",
};

void clubins_sht4x_driver_new(clubins_sht4x_driver_t *driver)
{
    if (driver == NULL) {
        return;
    }
    driver->initialised = false;
}

static clubins_sensor_status_t sht4x_init(clubins_sensor_t *self)
{
    clubins_sht4x_driver_t *driver = self->ctx;
    if (driver->initialised) {
        return CLUBINS_SENSOR_ERR_ALREADY_INITIALISED;
    }
    driver->initialised = true;
    return CLUBINS_SENSOR_OK;
}

static bool sht4x_sample(clubins_sensor_t *self, clubins_measurement_t *out)
{
    (void)out;
    const clubins_sht4x_driver_t *driver = self->ctx;
    if (!driver->initialised) {
        return false;
    }
    /* No readings yet: the hardware front-end is the bus task's deliverable
     * and samples nothing until it lands. */
    return false;
}

const clubins_sensor_ops_t clubins_sht4x_sensor_ops = {
    .meta = &clubins_sht4x_meta,
    .init = sht4x_init,
    .sample = sht4x_sample,
};
