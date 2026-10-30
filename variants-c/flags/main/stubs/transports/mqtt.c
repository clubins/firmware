//! Stub: MQTT v2 protobuf transport — the api/mqtt contract.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_TRANSPORT_MQTT; topic
//! v2/sensors/<uuid>/data/<type> at QoS 1, retained status on connect,
//! boot_id from the hardware RNG, per-topic seq from 0, clean-disconnect
//! `Offline`, LWT `Died`. Protobuf encoders use nanopb when the real
//! transport lands.

#include "stubkit.h"

void clubins_stub_mqtt_init(void)
{
    /* Real esp-mqtt client bring-up lands with the MQTT transport task. */
}

void clubins_stub_mqtt_tick(void)
{
    /* Real per-topic publish lifecycle lands with that task. */
}
