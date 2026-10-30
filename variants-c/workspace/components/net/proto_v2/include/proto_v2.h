//! MQTT v2 protobuf encoders (the `proto-v2` capability) —
//! the C port of the proto-v2 crate's scaffold. Will encode
//! api/mqtt/v2/*.proto messages with nanopb: per-type topics
//! (topics.md), seq starting at 0, stable boot_id, retained
//! status. The generated encoders land with the MQTT
//! transport task.

#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Stable boot identifier seed handling lands with the
 * MQTT transport task (topics.md's stable boot_id); the
 * scaffold exposes nothing yet. */
/** Bring the capability up once at boot (the composition calls this for
 * every composed hook, in registry order). */
void clubins_proto_v2_init(void);

/** Periodic hook: one slice of the capability's background work per
 * composition tick (the composition calls this for every composed hook,
 * after sampling the sensors). */
void clubins_proto_v2_tick(void);

#ifdef __cplusplus
}
#endif
