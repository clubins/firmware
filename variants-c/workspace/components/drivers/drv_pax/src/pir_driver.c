//! PIR driver implementation — the port of the driver-pax crate:
//! motion events fold over the sampling window into one `pax`
//! measurement, then the counter resets (the take in V2's sample()).

#include "pir_driver.h"

#include <limits.h>

const clubins_sensor_meta_t clubins_pir_meta = {
    .name = "PIR motion front-end",
    .ingest_type = "pax",
    .unit = "detections",
};

void clubins_pir_driver_new(clubins_pir_driver_t *driver)
{
    if (driver == NULL) {
        return;
    }
    driver->gpio_pin = CLUBINS_PIR_DEFAULT_GPIO_PIN;
    driver->motions_since_sample = 0;
    driver->initialised = false;
}

uint8_t clubins_pir_gpio_pin(const clubins_pir_driver_t *driver)
{
    return (driver == NULL) ? 0 : driver->gpio_pin;
}

void clubins_pir_record_motion(clubins_pir_driver_t *driver)
{
    if (driver == NULL || driver->motions_since_sample == UINT32_MAX) {
        return; /* saturating: an interrupt storm must never wrap */
    }
    driver->motions_since_sample++;
}

static clubins_sensor_status_t pir_init(clubins_sensor_t *self)
{
    clubins_pir_driver_t *driver = self->ctx;
    if (driver->initialised) {
        return CLUBINS_SENSOR_ERR_ALREADY_INITIALISED;
    }
    driver->initialised = true;
    return CLUBINS_SENSOR_OK;
}

static bool pir_sample(clubins_sensor_t *self, clubins_measurement_t *out)
{
    clubins_pir_driver_t *driver = self->ctx;
    if (!driver->initialised || driver->motions_since_sample == 0) {
        return false;
    }
    /* One measurement per window: the window's detections, clamped into
     * the model's uint8 occupancy ceiling (the 32-bit counter is
     * interrupt-side headroom; the wire field is the realistic window
     * maximum — C1's fixed model decision, kept). */
    out->type = CLUBINS_MEAS_PAX;
    out->as.pax.occupancy_count = (driver->motions_since_sample > UINT8_MAX)
                                      ? UINT8_MAX
                                      : (uint8_t)driver->motions_since_sample;
    driver->motions_since_sample = 0;
    return true;
}

const clubins_sensor_ops_t clubins_pir_sensor_ops = {
    .meta = &clubins_pir_meta,
    .init = pir_init,
    .sample = pir_sample,
};
