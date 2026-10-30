//! Stub: PIR occupancy front-end on a config-chosen GPIO pin.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_SENSOR_PAX; reports PIR motion
//! detections honestly — not a head count. Emits `pax`
//! (occupancy_count). No PIR hardware is documented in the source repo;
//! the pin convention lands with the pax driver task.

#include "stubkit.h"

void clubins_stub_pir_init(void)
{
    /* Real GPIO interrupt front-end lands with the pax driver task. */
}

void clubins_stub_pir_tick(void)
{
    /* Real debounce/occupancy assembly lands with the pax driver task. */
}
