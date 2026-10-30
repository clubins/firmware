//! Device entry point for the ESP32-S3 (V8 declarative pipeline engine).
//!
//! The boot path IS the architecture: parse the embedded pipelines document,
//! validate + instantiate it (a bad declaration fails the boot loudly), then
//! run the forever loop driving the cadence scheduler. The hardware buses
//! behind the source/stage/sink kinds land with the downstream driver PRs;
//! hardware sources are `Pending` until then — their pipelines validate,
//! schedule, and idle.
//!
//! Host builds of the bin target stay std on purpose: `cargo run` executes
//! the same boot path over a simulated time horizon and reports what each
//! pipeline delivered, so pipeline edits can be sanity-checked without the
//! Xtensa toolchain.
#![cfg_attr(target_arch = "xtensa", no_std, no_main)]

use firmware_v8_declarative::decl::Pipelines;

// The engine only exists when a sink kind does (see lib.rs).
#[cfg(any(
    feature = "transport-http",
    feature = "transport-mqtt",
    feature = "ops-status",
    feature = "logging-defmt"
))]
use firmware_v8_declarative::engine::Engine;

/// The pipelines document is compiled into the binary as the boot defaults;
/// a TOML edit retunes the graph, cargo features gate which kinds exist.
const PIPELINES_TOML: &str = include_str!("../config/pipelines.toml");

/// Sink-less build (no transport, status, or log sink compiled in): the
/// interpreter layer is compiled out — no pipeline declaration can
/// validate — so the on-target entry point just idles.
#[cfg(all(
    target_arch = "xtensa",
    not(any(
        feature = "transport-http",
        feature = "transport-mqtt",
        feature = "ops-status",
        feature = "logging-defmt"
    ))
))]
#[esp_hal::main]
fn main() -> ! {
    let _ = PIPELINES_TOML; // the document still compiles in; nothing runs it
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(all(
    target_arch = "xtensa",
    any(
        feature = "transport-http",
        feature = "transport-mqtt",
        feature = "ops-status",
        feature = "logging-defmt"
    )
))]
#[esp_hal::main]
fn main() -> ! {
    // The framework allocates (validation graphs, run state, streams):
    // register the heap region before anything runs. 64 KiB matches the
    // shared firmware stack convention; esp-alloc's default
    // `global-allocator` feature provides the allocator itself.
    esp_alloc::heap_allocator!(size: 65536);

    // Boot: declarations → validated instances → runnable pipelines, in
    // dependency order. Both failure modes abort the boot here, loudly,
    // rather than limping along with a half-built graph.
    let mut engine = match Pipelines::parse(PIPELINES_TOML) {
        Ok(doc) => match Engine::instantiate(&doc) {
            Ok(engine) => engine,
            Err(err) => panic!("pipeline graph invalid: {err}"),
        },
        Err(_) => panic!("pipelines.toml does not parse"),
    };

    // The runtime loop: nothing but the interpreter. Timer-driven hardware
    // sampling and real stage behaviour arrive with the driver PRs; until
    // then the loop proves the schedule and execution bookkeeping run
    // on-target.
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

#[cfg(all(
    not(target_arch = "xtensa"),
    any(
        feature = "transport-http",
        feature = "transport-mqtt",
        feature = "ops-status",
        feature = "logging-defmt"
    )
))]
fn main() {
    // Host: run the same boot path, then execute a simulated horizon of
    // three beats of the slowest pipeline and report per-pipeline outcomes.
    let doc =
        Pipelines::parse(PIPELINES_TOML).expect("committed pipelines.toml must parse");
    let mut engine =
        Engine::instantiate(&doc).expect("committed pipelines.toml must instantiate");
    println!("v8 declarative engine — {} pipeline(s):", engine.len());

    let horizon = 3 * engine.schedule().iter().map(|(_, c)| *c).max().unwrap_or(1);
    for ms in 0..=horizon {
        engine.tick(ms);
    }

    println!(
        "after {horizon} ms of simulated time (hardware sources are Pending — driver PRs pending):"
    );

    for (name, cadence_ms) in engine.schedule() {
        println!(
            "  {name:<16} every {cadence_ms:>6} ms — delivered {}, dropped {} (outbox {})",
            engine.delivered(name).unwrap_or(0),
            engine.dropped(name).unwrap_or(0),
            engine.outbox_dropped(name).unwrap_or(0),
        );
    }
}

/// Sink-less host build: the interpreter is compiled out, but the document
/// still parses — report the declaration count and stop.
#[cfg(all(
    not(target_arch = "xtensa"),
    not(any(
        feature = "transport-http",
        feature = "transport-mqtt",
        feature = "ops-status",
        feature = "logging-defmt"
    ))
))]
fn main() {
    let doc =
        Pipelines::parse(PIPELINES_TOML).expect("committed pipelines.toml must parse");
    println!(
        "v8 declarative engine — {} pipeline declaration(s), but no sink kind is \
         compiled in; nothing to run",
        doc.pipelines.len()
    );
}
