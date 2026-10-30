//! Stub: BH1750 — ambient light over I2C.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_SENSOR_BH1750; emits `light` (lx) —
//! a new measurement type, one-topic-per-type rule.

#include "stubkit.h"

void clubins_stub_bh1750_init(void)
{
    /* Real I2C bring-up lands with the BH1750 driver task. */
}

void clubins_stub_bh1750_tick(void)
{
    /* Real lux conversion lands with the BH1750 driver task. */
}
