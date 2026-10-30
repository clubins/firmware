//! Clubins firmware — V6 static lock-free dataflow variant.
//!
//! One crate, two personalities (spec §Architecture): the pure-logic modules
//! here compile and test on the host (the fast CI path); everything
//! hardware-bound — real DMA engines, I2C/UART buses, Wi-Fi, NVS — lives in
//! the target-only bin ([`crate`]'s sibling `main.rs`) and in `esp-hal`,
//! declared as a dependency for the Xtensa target only.
//!
//! Architecture (V6): the system is a statically-allocated graph of lock-free
//! SPSC ring buffers. DMA engines (I2S/PDM, UART) push bytes directly into
//! rings; processing stages drain rings with zero heap and no locks;
//! transports are graph drains; the memory budget is fixed at link time.
//! Each of the 15 public feature flags (spec §Feature Matrix) compiles in
//! one node module under [`sensors`], [`transports`], or [`ops`] — any
//! combination must compile (`just flag-matrix` verifies all-off, each flag
//! alone, all-on).
//!
//! Module map:
//! - [`ring`] — the universal primitive: lock-free SPSC ring, counted
//!   reject-newest overflow, consumer-side skip/lag.
//! - [`cell`] — one-time init `StaticCell` for pinning the graph.
//! - [`measurement`] — the wire-shape `Measurement` enum + ingest names.
//! - [`dma`] — simulated DMA engines (I2S/PDM, UART) + GPIO ISR producer.
//! - [`nodekit`] — shared node machinery (measurement rings, `offer`).
//! - [`sensors`]/[`transports`]/[`ops`] — the 15 flag-gated node modules.

#![cfg_attr(not(test), no_std)]

pub mod cell;
pub mod dma;
pub mod measurement;
pub mod nodekit;
pub mod ring;

pub mod sensors {
    //! Sensor source nodes — one module per `sensor-*` flag. Real drivers
    //! ship in downstream PRs (docs/plan/build-dag.md).

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
    //! Ingest transport drain nodes — config selects the active one at
    //! runtime; these flags decide what is compiled in at all.

    #[cfg(feature = "transport-http")]
    pub mod http;
    #[cfg(feature = "transport-mqtt")]
    pub mod mqtt;
    #[cfg(feature = "transport-mtls")]
    pub mod mtls;
}

pub mod ops {
    //! Ops capability nodes — OTA, provisioning, sleep, logging, status.

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
