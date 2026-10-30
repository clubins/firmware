//! Stub: deep-sleep duty cycling — trades measurement continuity for
//! power.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_DEEP_SLEEP; esp_sleep based.

#include "stubkit.h"

void clubins_stub_deep_sleep_init(void)
{
    /* Real sleep-scheduler bring-up lands with the deep-sleep task. */
}

void clubins_stub_deep_sleep_tick(void)
{
    /* Real duty-cycle transitions land with the deep-sleep task. */
}
