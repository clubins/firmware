//! BLE provisioning capability (the `provisioning` flag) —
//! the C port of the ops-provisioning crate's scaffold.
//! Will advertise, take Wi-Fi credentials and sensor
//! identity over NimBLE, and hand them to the config pool.
//! The provisioning flow lands with the provisioning tasks.

#include "ops_provisioning.h"

void clubins_ops_provisioning_init(void)
{
    /* Lifecycle stub: the real bring-up lands with the ops-provisioning port's
     * implementation tasks. */
}

void clubins_ops_provisioning_tick(void)
{
    /* Tick stub: the real background work lands with the ops-provisioning port's
     * implementation tasks. */
}
