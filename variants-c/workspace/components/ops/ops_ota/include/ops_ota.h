//! OTA update capability (the `ota` flag) — the C port of the
//! ops-ota crate's scaffold. Will pull signed updates over
//! esp_https_ota, verify them with mbedtls (mTLS pinned
//! profile), and hand control to esp_ota_mark_valid. The
//! update state machine lands with the OTA tasks.

#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Update flow lands with the OTA tasks: the scaffold
 * exposes only the composition lifecycle. */
/** Bring the capability up once at boot (the composition calls this for
 * every composed hook, in registry order). */
void clubins_ops_ota_init(void);

/** Periodic hook: one slice of the capability's background work per
 * composition tick (the composition calls this for every composed hook,
 * after sampling the sensors). */
void clubins_ops_ota_tick(void);

#ifdef __cplusplus
}
#endif
