//! Deep-sleep power capability (the `deep-sleep` flag) — the
//! C port of the ops-sleep crate's scaffold. Will park the
//! ESP32-S3 in esp_sleep deep sleep between measurement
//! windows and reboot through the composition on wake. The
//! sleep/wake policy lands with the power tasks.

#include "ops_sleep.h"

void clubins_ops_sleep_init(void)
{
    /* Lifecycle stub: the real bring-up lands with the ops-sleep port's
     * implementation tasks. */
}

void clubins_ops_sleep_tick(void)
{
    /* Tick stub: the real background work lands with the ops-sleep port's
     * implementation tasks. */
}
