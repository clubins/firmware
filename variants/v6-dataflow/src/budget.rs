//! Fixed memory-budget accounting.
//!
//! V6's budget is fixed at link time because nothing is heap-allocated:
//! the whole system is one `static` (the [`crate::graph::Graph`]), and
//! `size_of::<Graph>()` — evaluated per feature set at compile time — is
//! its entire RAM footprint (plus the pump's transient stack). The const
//! assertion in [`crate::system`] fails the build for whichever feature
//! set exceeds the budget; the flag-matrix CI recipe runs it for every
//! combination (all-off, each flag alone, all-on).

/// Fixed budget for the statically-allocated graph, in bytes. Sized well
/// under the ESP32-S3's SRAM so every flag combination fits — including
/// all-features — with room for the networking PRs' static buffers.
pub const BUDGET_BYTES: usize = 64 * 1024;

/// The compiled graph's footprint (bytes) for the active feature set.
pub const GRAPH_BYTES: usize = crate::graph::Graph::BYTES;

/// Budget headroom for the active feature set (bytes).
pub const HEADROOM_BYTES: usize = BUDGET_BYTES - GRAPH_BYTES;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_graph_fits_the_budget() {
        // Compile-time facts: checked statically, not at test runtime.
        const {
            assert!(
                GRAPH_BYTES <= BUDGET_BYTES,
                "graph exceeds its memory budget"
            );
            assert!(HEADROOM_BYTES > 0);
        }
    }

    /// The declared footprint covers every node the graph owns (the cfg
    /// sum below mirrors `graph.rs`'s feature-gated fields).
    #[test]
    #[allow(unused_mut)] // all-off: no nodes are compiled to count
    fn graph_footprint_covers_its_nodes() {
        let mut nodes = 0;
        #[cfg(feature = "sensor-decibel")]
        {
            nodes += crate::sensors::decibel::Node::BYTES;
        }
        #[cfg(feature = "sensor-pms5003")]
        {
            nodes += crate::sensors::pms5003::Node::BYTES;
        }
        #[cfg(feature = "sensor-pax")]
        {
            nodes += crate::sensors::pax::Node::BYTES;
        }
        #[cfg(feature = "sensor-bme280")]
        {
            nodes += crate::sensors::bme280::Node::BYTES;
        }
        #[cfg(feature = "sensor-sht4x")]
        {
            nodes += crate::sensors::sht4x::Node::BYTES;
        }
        #[cfg(feature = "sensor-scd4x")]
        {
            nodes += crate::sensors::scd4x::Node::BYTES;
        }
        #[cfg(feature = "sensor-bh1750")]
        {
            nodes += crate::sensors::bh1750::Node::BYTES;
        }
        #[cfg(feature = "transport-http")]
        {
            nodes += crate::transports::http::Node::BYTES;
        }
        #[cfg(feature = "transport-mqtt")]
        {
            nodes += crate::transports::mqtt::Node::BYTES;
        }
        #[cfg(feature = "transport-mtls")]
        {
            nodes += crate::transports::mtls::Node::BYTES;
        }
        #[cfg(feature = "ops-status")]
        {
            nodes += crate::ops::status::Node::BYTES;
        }
        #[cfg(feature = "ota")]
        {
            nodes += crate::ops::ota::Node::BYTES;
        }
        #[cfg(feature = "provisioning")]
        {
            nodes += crate::ops::provisioning::Node::BYTES;
        }
        #[cfg(feature = "deep-sleep")]
        {
            nodes += crate::ops::deep_sleep::Node::BYTES;
        }
        #[cfg(feature = "logging-defmt")]
        {
            nodes += crate::ops::logging::Node::BYTES;
        }
        // Nodes dominate the graph's size; a build with nodes must
        // account for at least their sum.
        assert!(GRAPH_BYTES >= nodes);
    }
}
