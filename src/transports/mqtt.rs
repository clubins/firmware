//! Stub: MQTT v2 protobuf client per the `api/mqtt` design — one topic per
//! measurement type (`v2/sensors/<uuid>/data/<type>`, QoS 1), retained
//! `Online` status on connect, `Offline` on clean disconnect, LWT `Died`;
//! `boot_id` from the hardware RNG per boot, `seq` from 0 per topic per
//! boot; mTLS client cert CN = sensor UUID.
//!
//! Compiled in by `feature = "transport-mqtt"`; the real transport ships in
//! a downstream PR (docs/plan/build-dag.md), conformance-tested against the
//! `.proto` files on the host.

/// Module marker for the presence test.
pub const MODULE: &str = "transports::mqtt";
