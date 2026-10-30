//! Stub: defmt logging transport (UART / USB-JTAG on the ESP32-S3).
//!
//! Compiled in by `feature = "logging-defmt"`; the real wiring (a
//! `defmt`-enabled logging init for the Xtensa target) ships in a downstream
//! PR (docs/plan/build-dag.md). Kept as a stub in the scaffold so the flag
//! compiles everywhere without pinning the channel crate early — note that
//! `defmt-rtt` is cortex-m-only and wrong for the ESP32-S3.

/// Module marker for the presence test.
pub const MODULE: &str = "ops::logging_defmt";
