//! Stub: HTTP-JSON ingest transport — the working server path.
//!
//! Compiled in by CONFIG_CLUBINS_CAP_TRANSPORT_HTTP; Wi-Fi STA plus HTTPS
//! POST of one JSON object per measurement to
//! <endpoint>/v1/sensors/<type>, field-exact against the Go ingest
//! structs (sensor_id, apikey, time, measurement_id, dba/dbc, ...).
//! API key from NVS, never from the committed config.

#include "stubkit.h"

void clubins_stub_http_init(void)
{
    /* Real Wi-Fi bring-up + esp_http_client lands with the HTTP task. */
}

void clubins_stub_http_tick(void)
{
    /* Real drain-and-publish lands with the HTTP transport task. */
}
