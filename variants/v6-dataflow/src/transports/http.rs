//! HTTP transport drain node — gated by the `transport-http` flag (pulls
//! `net-wifi`).
//!
//! Driver contract (spec §Transports; HTTP-JSON is the working server
//! path): Wi-Fi STA, then HTTPS POST of one JSON object per measurement to
//! `<endpoint>/v1/sensors/<type>` with fields exactly as the Go structs
//! define them (`sensor_id`, `apikey`, `time`, `measurement_id`, plus the
//! type's fields: `dba`/`dbc`, `occupancy_count`, weather fields, status
//! fields). The API key comes from NVS, never from committed config;
//! failures back off exponentially and drain a bounded offline buffer when
//! connectivity returns. In V6 this node is a graph **drain**: it owns an
//! input measurement ring and drains it each step — the Wi-Fi + TLS + JSON
//! serialization stack lands with the network PR; the scaffold counts what
//! it would send.

use crate::nodekit::MeasRing;

/// Ingest type name — the transport itself has no measurement type; it
/// publishes whatever reaches its ring.
pub const TRANSPORT_NAME: &str = "http";

/// The HTTP drain node: measurement ring in, wire out (scaffold: counted).
pub struct Node {
    /// Measurements awaiting publication, fed by the graph router.
    pub in_ring: MeasRing,
    sent: u64,
}

impl Default for Node {
    fn default() -> Self {
        Self::new()
    }
}

impl Node {
    pub const fn new() -> Self {
        Self {
            in_ring: MeasRing::new(),
            sent: 0,
        }
    }

    /// Measurements drained since boot.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Static footprint of this node (state + ring), for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step: drain the input ring. Each measurement is one
    /// HTTPS POST body; the network PR replaces the count with the send
    /// (and moves undeliverable ones to the bounded offline buffer).
    /// Returns measurements drained this step.
    pub fn poll(&mut self) -> usize {
        let mut drained = 0usize;
        while self.in_ring.pop().is_some() {
            drained += 1;
        }
        self.sent += drained as u64;
        drained
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Measurement;

    #[test]
    fn drains_everything_queued() {
        let mut node = Node::new();
        for i in 0..8u16 {
            assert!(node.in_ring.push(Measurement::Co2 { ppm: i }).is_ok());
        }
        assert_eq!(node.poll(), 8);
        assert_eq!(node.sent(), 8);
        assert!(node.in_ring.is_empty());
        assert_eq!(node.poll(), 0);
        assert_eq!(node.sent(), 8);
    }

    #[test]
    fn drained_measurement_types_are_untouched() {
        // The drain is type-agnostic: whatever reaches the ring is counted
        // out; the network PR only serializes it.
        let mut node = Node::new();
        assert!(
            node.in_ring
                .push(Measurement::Decibel { dba: 412, dbc: 462 })
                .is_ok()
        );
        assert_eq!(node.poll(), 1);
        match node.in_ring.pop() {
            None => {} // drained
            Some(m) => panic!("drained but still present: {m:?}"),
        }
    }
}
