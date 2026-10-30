//! Zero-heap proof for the V6 static lock-free dataflow.
//!
//! The architecture's claim (spec §Architecture): the system allocates
//! nothing — the whole graph is statically allocated, so there is zero
//! heap use at init and after init. This test installs a counting global
//! allocator that attributes bytes to the allocating *thread*, and asserts
//! the counter does not move across graph construction and pumping (the
//! graph pump runs entirely on one thread). Thread attribution is what
//! makes the measurement exact: the test harness allocates on its own
//! threads (spawn packets, capture buffers), which must not count against
//! the graph. A positive control proves the counter can see an allocation
//! on the measuring thread, so a silent no-op allocator cannot fake a pass.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use firmware_v6_dataflow::cell::StaticCell;
use firmware_v6_dataflow::graph::Graph;
use firmware_v6_dataflow::system;

thread_local! {
    /// Heap bytes allocated by this thread. Const-initialized, so the
    /// first access does not itself allocate.
    static THREAD_BYTES: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        THREAD_BYTES.with(|b| b.set(b.get() + layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// Heap bytes allocated by the calling thread.
fn thread_allocated() -> usize {
    THREAD_BYTES.with(Cell::get)
}

#[test]
fn the_counter_sees_a_real_allocation() {
    let before = thread_allocated();
    let probe = Vec::<u8>::with_capacity(4096);
    assert_eq!(thread_allocated() - before, 4096);
    drop(probe);
}

#[test]
fn graph_build_and_pump_allocate_zero_heap() {
    let before = thread_allocated();
    let mut graph = system::build();
    for _ in 0..1000 {
        graph.step();
    }
    let _ = graph.tick();
    assert_eq!(
        thread_allocated(),
        before,
        "graph build + pump allocated heap bytes"
    );
}

#[test]
fn the_pinned_static_graph_allocates_zero_heap() {
    static PINNED: StaticCell<Graph> = StaticCell::new();
    let before = thread_allocated();
    let graph = PINNED.init(system::build());
    for _ in 0..1000 {
        graph.step();
    }
    assert_eq!(
        thread_allocated(),
        before,
        "StaticCell init + pump allocated heap bytes"
    );
}
