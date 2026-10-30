//! MQTT v2 transport (the `transport-mqtt` capability) — the
//! C port of the transport-mqtt crate's scaffold. Will run
//! the esp-mqtt client against the v2 topic map (topics.md),
//! encoding proto-v2 payloads (nanopb), seq from 0, stable
//! boot_id, retained status. The client task and reconnection
//! policy land with the MQTT transport tasks; REQUIRES the
//! proto_v2 component (V2 parity).

#include "transport_mqtt.h"

void clubins_transport_mqtt_init(void)
{
    /* Lifecycle stub: the real bring-up lands with the transport-mqtt port's
     * implementation tasks. */
}

void clubins_transport_mqtt_tick(void)
{
    /* Tick stub: the real background work lands with the transport-mqtt port's
     * implementation tasks. */
}
