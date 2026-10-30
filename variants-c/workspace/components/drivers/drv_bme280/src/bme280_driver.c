//! BME280 driver implementation — the scaffold of the driver-bme280
//! crate: lifecycle and metadata now, the I2C front-end with the bus
//! task.

#include "bme280_driver.h"

const clubins_sensor_meta_t clubins_bme280_meta = {
    .name = "BME280",
    .ingest_type = "bme280",
    .unit = "°C, %RH, hPa",
};

void clubins_bme280_driver_new(clubins_bme280_driver_t *driver)
{
    if (driver == NULL) {
        return;
    }
    driver->initialised = false;
}

static clubins_sensor_status_t bme280_init(clubins_sensor_t *self)
{
    clubins_bme280_driver_t *driver = self->ctx;
    if (driver->initialised) {
        return CLUBINS_SENSOR_ERR_ALREADY_INITIALISED;
    }
    driver->initialised = true;
    return CLUBINS_SENSOR_OK;
}

static bool bme280_sample(clubins_sensor_t *self, clubins_measurement_t *out)
{
    (void)out;
    const clubins_bme280_driver_t *driver = self->ctx;
    if (!driver->initialised) {
        return false;
    }
    /* No readings yet: the hardware front-end is the bus task's deliverable
     * and samples nothing until it lands. */
    return false;
}

const clubins_sensor_ops_t clubins_bme280_sensor_ops = {
    .meta = &clubins_bme280_meta,
    .init = bme280_init,
    .sample = bme280_sample,
};
