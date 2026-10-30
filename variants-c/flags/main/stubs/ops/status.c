//! Stub: status heartbeat + boot lifecycle — required by both ingest
//! contracts (HTTP `status` payloads, MQTT v2 retained status).
//!
//! Compiled in by CONFIG_CLUBINS_CAP_OPS_STATUS; the heartbeat model
//! (subsystem, result, message) lands with the ops-status task.

#include "stubkit.h"

void clubins_stub_status_init(void)
{
    /* Real heartbeat bring-up lands with the ops-status task. */
}

void clubins_stub_status_tick(void)
{
    /* Real heartbeat emission lands with the ops-status task. */
}
