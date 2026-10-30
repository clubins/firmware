//! Sensor drivers — each module behind its own cargo flag.
//!
//! All drivers implement [`crate::sensor::Sensor`] and are built into the
//! registry at boot; a driver disabled in runtime config is absent instead of
//! polled. Stubs document exactly which task lands the hardware work.

#[cfg(feature = "sensor-bh1750")]
pub mod bh1750;
#[cfg(feature = "sensor-bme280")]
pub mod bme280;
#[cfg(feature = "sensor-decibel")]
pub mod decibel;
#[cfg(feature = "sensor-pax")]
pub mod pir;
#[cfg(feature = "sensor-pms5003")]
pub mod pms5003;
#[cfg(feature = "sensor-scd4x")]
pub mod scd4x;
#[cfg(feature = "sensor-sht4x")]
pub mod sht4x;
