//! BLE provisioning capability (the `provisioning` flag) —
//! the C port of the ops-provisioning crate's scaffold.
//! Will advertise, take Wi-Fi credentials and sensor
//! identity over NimBLE, and hand them to the config pool.
//! The provisioning flow lands with the provisioning tasks.

#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Provisioning flow lands with the provisioning tasks:
 * the scaffold exposes only the composition lifecycle. */
/** Bring the capability up once at boot (the composition calls this for
 * every composed hook, in registry order). */
void clubins_ops_provisioning_init(void);

/** Periodic hook: one slice of the capability's background work per
 * composition tick (the composition calls this for every composed hook,
 * after sampling the sensors). */
void clubins_ops_provisioning_tick(void);

#ifdef __cplusplus
}
#endif
