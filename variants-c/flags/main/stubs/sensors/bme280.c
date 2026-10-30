//! Stub: BME280 — temperature, humidity, pressure over I2C.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_SENSOR_BME280; emits `weather`
//! (temperature, humidity, pressure). Precipitation/rain/wind stay unset —
//! those are virtual Open-Meteo fields on the server.

#include "stubkit.h"

void clubins_stub_bme280_init(void)
{
    /* Real I2C bring-up lands with the BME280 driver task. */
}

void clubins_stub_bme280_tick(void)
{
    /* Real conversion-math sampling lands with the BME280 driver task. */
}
