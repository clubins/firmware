//! Stub: SHT4x — temperature and humidity over I2C.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_SENSOR_SHT4X; the alternative to
//! BME280's temp/hum — both compiled in means config picks per-device.
//! Emits `weather` (temperature, humidity).

#include "stubkit.h"

void clubins_stub_sht4x_init(void)
{
    /* Real I2C bring-up lands with the SHT4x driver task. */
}

void clubins_stub_sht4x_tick(void)
{
    /* Real conversion-math sampling lands with the SHT4x driver task. */
}
