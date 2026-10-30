//! Stub: mTLS client transport — client certificate and key from NVS, CN =
//! sensor UUID.
//!
//! Compiled in by `feature = "transport-mtls"` (implies `transport-mqtt`);
//! the real implementation ships in a downstream PR
//! (docs/plan/build-dag.md).

/// Module marker for the presence test.
pub const MODULE: &str = "ops::mtls";
