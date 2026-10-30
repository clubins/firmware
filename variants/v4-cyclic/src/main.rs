//! ESP32-S3 entry — thin by design: init config, build the registry and
//! schedule, run the executive loop. All logic lives in the library so the
//! host tests exercise the same code the target runs.
//!
//! The Xtensa personality gets `no_main` + the esp-hal entry attribute; the
//! host personality keeps a real `main` so `cargo build`/`test` link normally.

#![cfg_attr(target_arch = "xtensa", no_std)]
#![cfg_attr(target_arch = "xtensa", no_main)]

/// Minimal panic policy: halt and spin. The real handler (logging + watchdog
/// reset) lands with the logging task; the watchdog eventually fires either way.
#[cfg(target_arch = "xtensa")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(target_arch = "xtensa")]
#[esp_hal::main]
fn main() -> ! {
    // Real bring-up (timer interrupt, phase handlers, Wi-Fi, NVS) lands with
    // the hardware tasks; the executive owns the loop from the first tick.
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_arch = "xtensa"))]
fn main() {
    // Host builds compile the binary crate only; the library is the tested
    // surface (see tests/ for the executive suites).
}
