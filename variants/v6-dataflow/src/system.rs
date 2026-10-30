//! Feature-gated composition root.
//!
//! [`build`] returns the dataflow graph for the compiled feature set. On
//! device it is pinned into `static` storage with [`crate::cell::StaticCell`]
//! (see `main.rs`); the const assertion below holds the footprint to the
//! fixed budget at compile time — for the feature set being built, whose
//! worst case (all-features) the flag-matrix recipe builds explicitly.

use crate::budget;
use crate::graph::Graph;

/// Build the system graph for the compiled feature set (const: storable in
/// a `StaticCell`, no init-time allocation).
pub const fn build() -> Graph {
    Graph::build()
}

// Link-time guarantee: the statically-allocated graph fits the fixed
// memory budget in the compiled configuration (all-features is the worst
// case and is built by the flag-matrix recipe).
const _: () = assert!(
    Graph::BYTES <= budget::BUDGET_BYTES,
    "the static dataflow graph exceeds the fixed memory budget",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_graph_reports_its_footprint() {
        let g = build();
        let _ = g.tick();
        // Compile-time facts: checked statically, not at test runtime.
        const {
            assert!(Graph::BYTES > 0);
            assert!(Graph::BYTES <= budget::BUDGET_BYTES);
        }
    }
}
