//! Operations capabilities, each behind its own cargo flag.
//!
//! The scaffold compiles the surface and its invariants; behaviour lands
//! with the driver PRs (docs/plan/build-dag.md). Keep these modules honest:
//! a stub that pretends to flash is worse than one that documents what is
//! still missing.

#[cfg(feature = "deep-sleep")]
pub mod deep_sleep;
#[cfg(feature = "logging-defmt")]
pub mod logging;
#[cfg(feature = "ota")]
pub mod ota;
#[cfg(feature = "provisioning")]
pub mod provisioning;
