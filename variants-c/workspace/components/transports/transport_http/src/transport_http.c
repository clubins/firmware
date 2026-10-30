//! HTTP ingest transport (the `transport-http` capability) —
//! the C port of the transport-http crate's scaffold. Will
//! POST JSON ingest batches to
//! https://ingest.clubins.de/v1/sensors/<type> via
//! esp_http_client (mTLS when selected), mapping the server
//! status codes to connect/submit/batch outcomes. The
//! request task and retry policy land with the transport
//! tasks.

#include "transport_http.h"

void clubins_transport_http_init(void)
{
    /* Lifecycle stub: the real bring-up lands with the transport-http port's
     * implementation tasks. */
}

void clubins_transport_http_tick(void)
{
    /* Tick stub: the real background work lands with the transport-http port's
     * implementation tasks. */
}
