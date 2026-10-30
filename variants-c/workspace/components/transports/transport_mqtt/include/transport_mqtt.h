//! MQTT v2 transport (the `transport-mqtt` capability) — the
//! C port of the transport-mqtt crate's scaffold. Will run
//! the esp-mqtt client against the v2 topic map (topics.md),
//! encoding proto-v2 payloads (nanopb), seq from 0, stable
//! boot_id, retained status. The client task and reconnection
//! policy land with the MQTT transport tasks; REQUIRES the
//! proto_v2 component (V2 parity).

#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Publish path lands with the transport tasks: the
 * scaffold exposes only the composition lifecycle. */
/** Bring the capability up once at boot (the composition calls this for
 * every composed hook, in registry order). */
void clubins_transport_mqtt_init(void);

/** Periodic hook: one slice of the capability's background work per
 * composition tick (the composition calls this for every composed hook,
 * after sampling the sensors). */
void clubins_transport_mqtt_tick(void);

#ifdef __cplusplus
}
#endif
