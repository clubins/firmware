//! Device entry point for the ESP32-S3 (V8 declarative pipeline engine).
//!
//! The boot path IS the architecture: parse the embedded pipelines document,
//! validate + instantiate it (a bad declaration fails the boot loudly), then
//! run the forever loop driving the cadence scheduler. The hardware buses
//! behind the source/stage/sink kinds land with the downstream driver PRs;
//! this scaffold proves the graph instantiates and the schedule runs.
//!
//! Host builds of the bin target stay std on purpose: `cargo run` executes
//! the same boot path and prints the resolved schedule, so pipeline edits
//! can be sanity-checked without the Xtensa toolchain.
#![cfg_attr(target_arch = "xtensa", no_std, no_main)]

use firmware_v8_declarative::decl::Pipelines;
use firmware_v8_declarative::engine::Engine;

/// The pipelines document is compiled into the binary as the boot defaults;
/// a TOML edit retunes the graph, cargo features gate which kinds exist.
const PIPELINES_TOML: &str = include_str!("../config/pipelines.toml");

#[cfg(target_arch = "xtensa")]
#[esp_hal::main]
fn main() -> ! {
    // The framework allocates (validation graphs, the instance list): register
    // the heap region before anything runs. 64 KiB matches the shared
    // firmware stack convention; esp-alloc's default `global-allocator`
    // feature provides the allocator itself.
    esp_alloc::heap_allocator!(size: 65536);

    // Boot: declarations → validated instances, in dependency order. Both
    // failure modes abort the boot here, loudly, rather than limping along
    // with a half-built graph.
    let mut engine = match Pipelines::parse(PIPELINES_TOML) {
        Ok(doc) => match Engine::instantiate(&doc) {
            Ok(engine) => engine,
            Err(err) => panic!("pipeline graph invalid: {err}"),
        },
        Err(_) => panic!("pipelines.toml does not parse"),
    };

    // The runtime loop: nothing but the scheduler. Timer-driven hardware
    // sampling and stage execution arrive with the driver PRs; until then
    // the loop proves the schedule bookkeeping runs on-target.
    let mut now_ms: u64 = 0;
    loop {
        engine.tick(now_ms);
        now_ms += 1;
        core::hint::spin_loop();
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
    // Host: run the same boot path and print the resolved schedule.
    let doc =
        Pipelines::parse(PIPELINES_TOML).expect("committed pipelines.toml must parse");
    let engine =
        Engine::instantiate(&doc).expect("committed pipelines.toml must instantiate");
    println!("v8 declarative engine — {} pipeline(s):", engine.len());
    for (name, cadence_ms) in engine.schedule() {
        println!("  {name:<16} every {cadence_ms} ms");
    }
}
