//! Stub: HTTP JSON ingest — Wi-Fi STA, then HTTPS POST of one JSON object
//! per measurement to `<endpoint>/v1/sensors/<type>`.
//!
//! Compiled in by `feature = "transport-http"`; the real transport ships in
//! a downstream PR (docs/plan/build-dag.md) with JSON bodies field-exact
//! against the server's Go structs. The API key comes from NVS, never from
//! the committed config. Failures back off exponentially and drain a bounded
//! offline buffer on reconnect.

/// Module marker for the presence test.
pub const MODULE: &str = "transports::http";
