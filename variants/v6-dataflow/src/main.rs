//! Device entry point for the ESP32-S3 (V6 static lock-free dataflow).
//!
//! Stays thin per the spec (§Architecture): pin the statically-allocated
//! graph into `static` storage, then pump it — that loop is the whole
//! runtime. V6 registers no global allocator: the graph's rings and stages
//! are `static`-resident and allocate nothing at runtime; a downstream PR
//! that needs a heap (Wi-Fi/TLS bring-up) registers esp-alloc behind its
//! own flag. The real hardware sources (I2S/PDM and UART DMA, I2C polls,
//! GPIO interrupts) land with the driver PRs (docs/plan/build-dag.md); this
//! scaffold proves the Xtensa personality builds and links with the graph
//! resident in a `static`.
//!
//! Host builds of the bin target stay std on purpose: they exist so `cargo
//! check` and `cargo test` are green on machines without the Xtensa target.
#![cfg_attr(target_arch = "xtensa", no_std, no_main)]

#[cfg(target_arch = "xtensa")]
#[esp_hal::main]
fn main() -> ! {
    use firmware_v6_dataflow::{cell::StaticCell, graph::Graph};

    // The statically-allocated dataflow graph: no heap, no allocator, no
    // locks. The cell pins the graph in `static` storage with a link-time-
    // fixed footprint (checked against the budget at compile time — see
    // budget.rs); the pump below is the whole runtime.
    static GRAPH: StaticCell<Graph> = StaticCell::new();
    let graph = GRAPH.init(Graph::build());

    loop {
        graph.step();
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
    // Host stub — the device personality only builds for the ESP32-S3. The
    // architecture itself (rings, DMA simulation, graph pump, budget) is
    // host-tested through the lib target.
}
