//! Device personality entry point.
//!
//! Host builds compile only a stub binary: every hardware and runtime
//! dependency is scoped to `cfg(target_arch = "xtensa")` in `Cargo.toml`, so
//! the pure logic in the library builds and tests anywhere. The real firmware
//! boots esp-hal, starts the esp-rtos embassy executor and spawns one task
//! per enabled capability (the tested mirror of the spawn arms is
//! `features::planned_tasks`).
#![cfg_attr(target_arch = "xtensa", no_std)]
#![cfg_attr(target_arch = "xtensa", no_main)]

#[cfg(target_arch = "xtensa")]
mod device;

/// Host builds have no device to run; the library is the deliverable here.
#[cfg(not(target_arch = "xtensa"))]
fn main() {
    println!(
        "clubins-firmware-v3: the device personality builds for xtensa-esp32s3-none-elf"
    );
    println!("host personality: pure logic lives in the library; `cargo test` covers it");
}
