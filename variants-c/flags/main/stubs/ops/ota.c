//! Stub: OTA updates via the esp-ota partition scheme.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_OTA; verify, apply, roll back.

#include "stubkit.h"

void clubins_stub_ota_init(void)
{
    /* Real esp_https_ota bring-up lands with the OTA task. */
}

void clubins_stub_ota_tick(void)
{
    /* Real update polling lands with the OTA task. */
}
