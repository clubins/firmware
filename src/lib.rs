//! Clubins venue-sensor firmware — host-testable core.
//!
//! One crate, two personalities (spec §Architecture): the pure-logic modules
//! here — sensor identity, feature-flag resolution, config/state anchors —
//! compile and test on the host; everything hardware-bound lives in the
//! target-only bin ([`src/main.rs`]) and in `esp-hal`, declared as a
//! dependency for the Xtensa target only.
//!
//! Every capability is a Cargo feature (spec §Feature Matrix): each
//! `sensor-*`, `transport-*`, and ops flag toggles its stub module under
//! [`sensors`], [`transports`], or [`ops`]. Any combination must compile —
//! CI verifies all-off, each flag alone, and all-on (`just flag-matrix`).

#![cfg_attr(not(test), no_std)]

pub mod config;
pub mod registry;
pub mod sensor;
pub mod state;

pub mod sensors {
    //! Sensor driver stubs — one module per `sensor-*` feature flag. The
    //! real drivers ship in downstream PRs (docs/plan/build-dag.md).

    #[cfg(feature = "sensor-bh1750")]
    pub mod bh1750;
    #[cfg(feature = "sensor-bme280")]
    pub mod bme280;
    #[cfg(feature = "sensor-decibel")]
    pub mod pdm_mic;
    #[cfg(feature = "sensor-pax")]
    pub mod pir;
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
    #[cfg(feature = "transport-mqtt")]
    pub mod mqtt;
}

pub mod ops {
    //! Ops stubs — OTA, provisioning, sleep, logging, mTLS, status.

    #[cfg(feature = "provisioning")]
    pub mod ble_provisioning;
    #[cfg(feature = "deep-sleep")]
    pub mod deep_sleep;
    #[cfg(feature = "logging-defmt")]
    pub mod logging_defmt;
    #[cfg(feature = "transport-mtls")]
    pub mod mtls;
    #[cfg(feature = "ota")]
    pub mod ota;
    #[cfg(feature = "ops-status")]
    pub mod status;
}
