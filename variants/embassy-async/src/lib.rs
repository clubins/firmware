//! Clubins ESP32-S3 firmware — variant C: embassy-async task architecture.
//!
//! One crate, two personalities:
//!
//! - The library (this tree, `no_std`) holds every pure-logic concern: the
//!   measurement/status event model, the typed bounded channels that connect
//!   tasks, the built-in configuration constants, and the feature-resolution
//!   layer that turns the cargo flag matrix into a task plan. It compiles
//!   and tests on the host, keeping CI fast.
//! - The device personality (`src/main.rs` + `src/device/`, Xtensa only)
//!   boots esp-hal, starts the esp-rtos embassy executor, and spawns one
//!   async task per enabled capability. Sensor tasks publish
//!   [`events::MeasurementEvent`]s, ops tasks supervise, and the active
//!   transport task consumes both event kinds — cooperating over the bounded
//!   channels in [`channels`] with send-side backpressure.
//!
//! Verification (no hardware required):
//!
//! ```text
//! nix-shell --pure --run 'just check'        # fmt + clippy -D warnings + host tests
//! nix-shell --pure --run 'just flag-matrix'  # all-off, each leaf alone, all-on
//! nix-shell --pure --run 'just xtensa'       # release ELF for xtensa-esp32s3-none-elf
//! ```

#![no_std]

pub mod channels;
pub mod config;
pub mod events;
pub mod features;
