//! Status reporting capability (the `ops-status` flag) — the
//! C port of the ops-status crate's scaffold. Will compose
//! the status payload (per-sensor health from the registry,
//! transport link state, power state) on the periodic
//! cadence and hand it to the active transport. The report
//! builder lands with the status tasks.

#include "ops_status.h"

void clubins_ops_status_init(void)
{
    /* Lifecycle stub: the real bring-up lands with the ops-status port's
     * implementation tasks. */
}

void clubins_ops_status_tick(void)
{
    /* Tick stub: the real background work lands with the ops-status port's
     * implementation tasks. */
}
