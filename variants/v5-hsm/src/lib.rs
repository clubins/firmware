//! Clubins firmware — V5 hierarchical state-machine variant.
//!
//! One crate, two personalities (spec §Architecture): the pure-logic modules
//! here — event model, FSM framework, dispatcher, system machine, typed
//! measurement model, capability stubs — compile and test on the host;
//! everything hardware-bound lives in
//! the target-only bin ([`crate::`]'s sibling `main.rs`) and in `esp-hal`,
//! declared as a dependency for the Xtensa target only.
//!
//! Architecture (V5, QP-style): every subsystem is a hierarchical state
//! machine ([`fsm::Hsm`]) over one central event dispatcher
//! ([`dispatcher::Dispatcher`]). No preemptive tasks, no superloop; deep
//! sleep is a top-level system state ([`system`]). Each of the 15 public
//! feature flags (spec §Feature Matrix) compiles in one stub module under
//! [`sensors`], [`transports`], or [`ops`] — any combination must compile
//! (`just flag-matrix` verifies all-off, each flag alone, all-on).

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod config;
pub mod dispatcher;
pub mod events;
pub mod fsm;
pub mod measurement;
pub mod nvs;
pub mod stubkit;
pub mod system;

pub mod sensors {
    //! Sensor capability stubs — one module per `sensor-*` flag. Real
    //! drivers ship in downstream PRs (docs/plan/build-dag.md).

    #[cfg(feature = "sensor-bh1750")]
    pub mod bh1750;
    #[cfg(feature = "sensor-bme280")]
    pub mod bme280;
    #[cfg(feature = "sensor-decibel")]
    pub mod decibel;
    #[cfg(feature = "sensor-pax")]
    pub mod pax;
    #[cfg(feature = "sensor-pms5003")]
    pub mod pms5003;
    #[cfg(feature = "sensor-scd4x")]
    pub mod scd4x;
    #[cfg(feature = "sensor-sht4x")]
    pub mod sht4x;
}

pub mod transports {
    //! Ingest transport stubs — config selects the active one at runtime;
    //! these flags decide what is compiled in at all.

    #[cfg(feature = "transport-http")]
    pub mod http;
    #[cfg(feature = "transport-http")]
    pub mod http_json;
    #[cfg(feature = "transport-mqtt")]
    pub mod mqtt;
    #[cfg(feature = "transport-mtls")]
    pub mod mtls;
}

pub mod ops {
    //! Ops capability stubs — OTA, provisioning, sleep, logging, status.

    #[cfg(feature = "deep-sleep")]
    pub mod deep_sleep;
    #[cfg(feature = "logging-defmt")]
    pub mod logging;
    #[cfg(feature = "ota")]
    pub mod ota;
    #[cfg(feature = "provisioning")]
    pub mod provisioning;
    #[cfg(feature = "ops-status")]
    pub mod status;
}
