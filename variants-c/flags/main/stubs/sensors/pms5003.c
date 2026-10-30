//! Stub: PMS5003 — PM2.5 particulates over UART.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_SENSOR_PMS5003; emits `pm25`
//! (µg/m³) — a new measurement type, one-topic-per-type rule.

#include "stubkit.h"

void clubins_stub_pms5003_init(void)
{
    /* Real UART bring-up lands with the PMS5003 driver task. */
}

void clubins_stub_pms5003_tick(void)
{
    /* Real frame parse/checksum lands with the PMS5003 driver task. */
}
