//! Clubins firmware — V8: the declarative pipeline engine.
//!
//! Sensor pipelines are declared in data (`config/pipelines.toml`) and
//! instantiated at boot by a small interpreter over a statically-typed,
//! feature-gated registry of stage kinds. Adding or retuning a sensor is a
//! TOML edit, not a recompile: cargo features gate which kinds exist;
//! runtime TOML gates which instances run.
//!
//! Layout:
//! - [`decl`] — typed view over the pipelines document (`no_std` parser);
//! - [`registry`] — the feature-gated kinds and their parameter schemas;
//! - [`graph`] — boot-time validation (kinds, params, refs, cycles, cadences);
//! - [`runtime`] — the execution state built per validated instance
//!   (source/stage/sink executors and per-pipeline stream buffers);
//! - [`engine`] — instantiation, `pipe` wiring, and the cadence scheduler
//!   that executes due pipelines;
//! - [`measurement`] — the typed measurement model a pipeline produces;
//! - [`http_json`] — the HTTP JSON ingest codec (`transport-http` feature),
//!   field-exact against the Go ingest structs;
//! - [`config`] — the device config: committed `config/default.toml`
//!   resolved over built-in constants (unknown keys tolerated with a warn
//!   list; serialization carries a secret-exclusion tripwire);
//! - [`nvs`] — the NVS override layer: the top precedence rung, a
//!   warn-and-skip posture, and the injectable override source (the
//!   flash-backed source impl is device-side, behind `esp32s3`);
//! - [`ops`] — flag-gated operations capabilities (OTA, provisioning, …).
//!
//! Pure logic throughout: everything here is `no_std`-compatible with
//! `alloc` and host-tested; only the driver PRs touch hardware.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod config;
pub mod decl;
pub mod graph;
pub mod measurement;
pub mod nvs;
pub mod ops;
pub mod registry;

/// The interpreter — [`engine`] + [`runtime`] — exists only when at least
/// one sink kind is compiled in: every valid pipeline declaration needs a
/// sink, so a sink-less build has no runnable graph and no instance can
/// exist. (`status` heartbeats are delivered by the `ops-status` sink.)
#[cfg(any(
    feature = "transport-http",
    feature = "transport-mqtt",
    feature = "ops-status",
    feature = "logging-defmt"
))]
pub mod engine;
#[cfg(any(
    feature = "transport-http",
    feature = "transport-mqtt",
    feature = "ops-status",
    feature = "logging-defmt"
))]
pub mod runtime;

/// The HTTP JSON ingest codec — exists only when the device has an HTTP
/// transport to feed it to.
#[cfg(feature = "transport-http")]
pub mod http_json;
