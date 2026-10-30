//! Device entry point for the ESP32-S3 (V7 dual-core partitioned).
//!
//! Partition map (spec §Architecture): this main runs on core 0 (PRO CPU) —
//! the network/ops core. Core 1 (APP CPU) runs sensing + DSP and is started
//! with esp-hal's `CpuControl::start_app_core`; both cores exchange
//! measurements and control only through the lock-free boundary in
//! [`firmware_v7_dualcore::boundary`]. The launch wiring lands with the
//! cross-core primitive commit; this entry compiles and links the partition
//! skeleton on the target.
//!
//! Host builds of the bin target stay std on purpose: they exist so `cargo
//! check` and `cargo test` are green on machines without the Xtensa target.

#![cfg_attr(target_arch = "xtensa", no_std, no_main)]

#[cfg(target_arch = "xtensa")]
#[esp_hal::main]
fn main() -> ! {
    // Wi-Fi and TLS (downstream PRs) allocate; the partitions themselves do
    // not — steady-state cross-core traffic is fixed-capacity queues.
    esp_alloc::heap_allocator!(size: 65536);

    let peripherals = esp_hal::init(esp_hal::Config::default());

    // V7 partition launch: split the boundary, start core 1 (sensing +
    // DSP) via CpuControl::start_app_core, and run core 0 (transports +
    // ops) here — firmware_v7_dualcore::boot never returns. Path note:
    // esp-hal 1.2.2 keeps the dual-core API at esp_hal::system.
    let cpu_control = esp_hal::system::CpuControl::new(peripherals.CPU_CTRL);
    firmware_v7_dualcore::boot(cpu_control)
}

#[cfg(target_arch = "xtensa")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // Intentionally minimal for the scaffold; defmt diagnostics arrive with
    // the logging-defmt bring-up (docs/plan/build-dag.md).
    let _ = info;
    loop {}
}

#[cfg(not(target_arch = "xtensa"))]
fn main() {
    // Host stub — the device personality only builds for the ESP32-S3. The
    // partitions and the cross-core boundary are host-tested through the
    // lib target.
}
