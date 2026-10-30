//! MQTT v2 protobuf encoders (the `proto-v2` capability) —
//! the C port of the proto-v2 crate's scaffold. Will encode
//! api/mqtt/v2/*.proto messages with nanopb: per-type topics
//! (topics.md), seq starting at 0, stable boot_id, retained
//! status. The generated encoders land with the MQTT
//! transport task.

#include "proto_v2.h"

void clubins_proto_v2_init(void)
{
    /* Lifecycle stub: the real bring-up lands with the proto-v2 port's
     * implementation tasks. */
}

void clubins_proto_v2_tick(void)
{
    /* Tick stub: the real background work lands with the proto-v2 port's
     * implementation tasks. */
}
