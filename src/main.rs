//! Device entry point for the ESP32-S3.
//!
//! Stays thin per the spec (§Architecture): init config, bring up the
//! feature-gated sensors and transport, run the loop. The real bring-up lands
//! with the downstream driver/transport PRs (docs/plan/build-dag.md); this
//! skeleton only proves the Xtensa personality builds and links.
//!
//! Host builds of the bin target stay std on purpose: they exist so `cargo
//! check` and `cargo test` are green on machines without the Xtensa target.
#![cfg_attr(target_arch = "xtensa", no_std, no_main)]

#[cfg(target_arch = "xtensa")]
#[esp_hal::main]
fn main() -> ! {
    // Scaffold loop. Capability modules arrive behind their flags in
    // downstream PRs; the unconditional hardware watchdog (spec §Ops
    // Features) is wired up with the first real bring-up.
    loop {}
}

#[cfg(target_arch = "xtensa")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // Intentionally minimal for the scaffold; diagnostics reporting arrives
    // with the logging-defmt bring-up PR (docs/plan/build-dag.md).
    let _ = info;
    loop {}
}

#[cfg(not(target_arch = "xtensa"))]
fn main() {
    // Host stub — the device personality only builds for the ESP32-S3.
}
