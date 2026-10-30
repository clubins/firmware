//! Device entry point for the ESP32-S3 (V5 hierarchical state-machine).
//!
//! Stays thin per the spec (§Architecture): build the dispatcher with every
//! feature-gated subsystem machine, run bring-up, then serve the event
//! queue. The real hardware sources (I2S capture, I2C/UART buses, Wi-Fi,
//! timers, NVS) land with the downstream driver PRs (docs/plan/build-dag.md);
//! this scaffold proves the Xtensa personality builds and links.
//!
//! Host builds of the bin target stay std on purpose: they exist so `cargo
//! check` and `cargo test` are green on machines without the Xtensa target.
#![cfg_attr(target_arch = "xtensa", no_std, no_main)]

#[cfg(target_arch = "xtensa")]
#[esp_hal::main]
fn main() -> ! {
    // The framework and every stub allocate (Vec, defer queues): register
    // the heap region before anything runs. 64 KiB matches the shared
    // firmware stack convention; esp-alloc's default `global-allocator`
    // feature provides the allocator itself.
    esp_alloc::heap_allocator!(size: 65536);

    // Composition root: the system machine plus every compiled capability.
    let mut dispatcher = firmware_v5_hsm::system::build();
    dispatcher.start_all();

    // The dispatch loop is the whole runtime: no superloop of tasks, only
    // events. On device, posts arrive from ISRs and timer callbacks once the
    // hardware bring-up PRs land; the scaffold's queue is fed by nothing, so
    // idle is a spin (the deep-sleep state parks the system for real).
    loop {
        if dispatcher.run_until_idle(16) == 0 {
            core::hint::spin_loop();
        }
    }
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
    // Host stub — the device personality only builds for the ESP32-S3. The
    // architecture itself (event model, FSM, dispatcher, system machine) is
    // host-tested through the lib target.
}
