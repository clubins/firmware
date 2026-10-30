//! HTTP ingest transport (the `transport-http` capability) —
//! the C port of the transport-http crate's scaffold. Will
//! POST JSON ingest batches to
//! https://ingest.clubins.de/v1/sensors/<type> via
//! esp_http_client (mTLS when selected), mapping the server
//! status codes to connect/submit/batch outcomes. The
//! request task and retry policy land with the transport
//! tasks.

#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Publish path lands with the transport tasks: the
 * scaffold exposes only the composition lifecycle. */
/** Bring the capability up once at boot (the composition calls this for
 * every composed hook, in registry order). */
void clubins_transport_http_init(void);

/** Periodic hook: one slice of the capability's background work per
 * composition tick (the composition calls this for every composed hook,
 * after sampling the sensors). */
void clubins_transport_http_tick(void);

#ifdef __cplusplus
}
#endif
