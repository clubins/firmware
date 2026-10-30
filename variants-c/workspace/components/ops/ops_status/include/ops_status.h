//! Status reporting capability (the `ops-status` flag) — the
//! C port of the ops-status crate's scaffold. Will compose
//! the status payload (per-sensor health from the registry,
//! transport link state, power state) on the periodic
//! cadence and hand it to the active transport. The report
//! builder lands with the status tasks.

#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Report builder lands with the status tasks: the
 * scaffold exposes only the composition lifecycle. */
/** Bring the capability up once at boot (the composition calls this for
 * every composed hook, in registry order). */
void clubins_ops_status_init(void);

/** Periodic hook: one slice of the capability's background work per
 * composition tick (the composition calls this for every composed hook,
 * after sampling the sensors). */
void clubins_ops_status_tick(void);

#ifdef __cplusplus
}
#endif
