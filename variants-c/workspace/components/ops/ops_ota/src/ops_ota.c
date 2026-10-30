//! OTA update capability (the `ota` flag) — the C port of the
//! ops-ota crate's scaffold. Will pull signed updates over
//! esp_https_ota, verify them with mbedtls (mTLS pinned
//! profile), and hand control to esp_ota_mark_valid. The
//! update state machine lands with the OTA tasks.

#include "ops_ota.h"

void clubins_ops_ota_init(void)
{
    /* Lifecycle stub: the real bring-up lands with the ops-ota port's
     * implementation tasks. */
}

void clubins_ops_ota_tick(void)
{
    /* Tick stub: the real background work lands with the ops-ota port's
     * implementation tasks. */
}
