//! Deep-sleep power capability (the `deep-sleep` flag) — the
//! C port of the ops-sleep crate's scaffold. Will park the
//! ESP32-S3 in esp_sleep deep sleep between measurement
//! windows and reboot through the composition on wake. The
//! sleep/wake policy lands with the power tasks.

#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/** The sleep/wake policy lands with the power tasks: the
 * scaffold exposes only the composition lifecycle. */
/** Bring the capability up once at boot (the composition calls this for
 * every composed hook, in registry order). */
void clubins_ops_sleep_init(void);

/** Periodic hook: one slice of the capability's background work per
 * composition tick (the composition calls this for every composed hook,
 * after sampling the sensors). */
void clubins_ops_sleep_tick(void);

#ifdef __cplusplus
}
#endif
