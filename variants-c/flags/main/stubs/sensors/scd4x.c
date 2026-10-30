//! Stub: SCD4x — CO2 concentration over I2C.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_SENSOR_SCD4X; emits `co2` — a new
//! measurement type following the one-topic-per-type rule (no server
//! receiver yet; spec §Sensor Expansion).

#include "stubkit.h"

void clubins_stub_scd4x_init(void)
{
    /* Real I2C bring-up lands with the SCD4x driver task. */
}

void clubins_stub_scd4x_tick(void)
{
    /* Real ppm sampling lands with the SCD4x driver task. */
}
