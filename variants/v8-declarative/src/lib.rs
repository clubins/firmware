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
//! - [`engine`] — instantiation and the cadence scheduler;
//! - [`measurement`] — the typed measurement model a pipeline produces;
//! - [`http_json`] — the HTTP JSON ingest codec (`transport-http` feature),
//!   field-exact against the Go ingest structs;
//! - [`ops`] — flag-gated operations capabilities (OTA, provisioning, …).
//!
//! Pure logic throughout: everything here is `no_std`-compatible with
//! `alloc` and host-tested; only the driver PRs touch hardware.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod decl;
pub mod engine;
pub mod graph;
pub mod measurement;
pub mod ops;
pub mod registry;

/// The HTTP JSON ingest codec — exists only when the device has an HTTP
/// transport to feed it to.
#[cfg(feature = "transport-http")]
pub mod http_json;
