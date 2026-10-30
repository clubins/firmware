//! Stub: status heartbeat + boot lifecycle — required by both ingest
//! contracts (HTTP `status` payloads, MQTT v2 retained status).
//!
//! Compiled in by `feature = "ops-status"`; the real implementation ships
//! in a downstream PR (docs/plan/build-dag.md).

/// Module marker for the presence test.
pub const MODULE: &str = "ops::status";
