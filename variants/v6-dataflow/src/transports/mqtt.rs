//! MQTT v2 transport drain node — gated by the `transport-mqtt` flag
//! (pulls `net-wifi` + `proto-v2`).
//!
//! Driver contract (spec §Transports; the source monorepo's
//! api/mqtt/topics.md): connect to the broker, publish a retained `Status`
//! with `Online` on every connect, then one message per measurement on
//! `v2/sensors/<uuid>/data/<type>` at QoS 1. `boot_id` is 16 bytes from the
//! hardware RNG per boot; `seq` restarts at 0 per topic per boot; a clean
//! disconnect publishes `Offline`; the LWT is `Died`. Protobuf encoding is
//! conformance-tested against the `.proto` files on the host. In V6 this
//! node is a graph **drain** like HTTP: the broker client + protobuf
//! encoder land with the network PR; the scaffold counts what it would
//! publish per topic.

use crate::measurement::{MEASUREMENT_TYPES, Measurement};
use crate::nodekit::MeasRing;

/// Ingest type name.
pub const TRANSPORT_NAME: &str = "mqtt";

/// The MQTT drain node: measurement ring in, wire out (scaffold: counted).
pub struct Node {
    /// Measurements awaiting publication, fed by the graph router.
    pub in_ring: MeasRing,
    /// One sequence number per measurement topic, reset to 0 at boot per
    /// topics.md. Index by the measurement type.
    seq: [u64; MEASUREMENT_TYPES],
    sent: u64,
}

impl Node {
    pub const fn new() -> Self {
        Self {
            in_ring: MeasRing::new(),
            seq: [0; MEASUREMENT_TYPES],
            sent: 0,
        }
    }

    /// Measurements drained since boot.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Current sequence number for a measurement type's topic (the next
    /// message on that topic would use this value).
    pub fn seq_of(&self, m: &Measurement) -> u64 {
        self.seq[m.type_index()]
    }

    /// Static footprint of this node (state + ring), for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step: drain the input ring. Each measurement is one
    /// protobuf message on its type's topic; the network PR replaces the
    /// count with the publish. Returns measurements drained this step.
    pub fn poll(&mut self) -> usize {
        let mut drained = 0usize;
        while let Some(m) = self.in_ring.pop() {
            // seq advances per topic, from 0, exactly as topics.md requires.
            self.seq[m.type_index()] = self.seq[m.type_index()].wrapping_add(1);
            drained += 1;
        }
        self.sent += drained as u64;
        drained
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_advances_per_topic_from_zero() {
        let mut node = Node::new();
        // Two decibel and one co2 measurement, interleaved.
        for _ in 0..2 {
            assert!(
                node.in_ring
                    .push(Measurement::Decibel { dba: 400, dbc: 450 })
                    .is_ok()
            );
        }
        assert!(node.in_ring.push(Measurement::Co2 { ppm: 600 }).is_ok());
        assert_eq!(node.poll(), 3);
        let db = Measurement::Decibel { dba: 400, dbc: 450 };
        assert_eq!(node.seq_of(&db), 2);
        assert_eq!(node.seq_of(&Measurement::Co2 { ppm: 600 }), 1);
        // Decibel's seq is independent of co2's: the next decibel is seq 2
        // (it was the third message on that topic).
        assert!(node.in_ring.push(db).is_ok());
        assert_eq!(node.poll(), 1);
        assert_eq!(node.seq_of(&db), 3);
        assert_eq!(node.seq_of(&Measurement::Co2 { ppm: 600 }), 1);
        assert_eq!(node.sent(), 4);
    }

    #[test]
    fn fresh_node_starts_every_topic_at_zero() {
        let node = Node::new();
        for i in 0..MEASUREMENT_TYPES {
            assert_eq!(node.seq[i], 0, "topic {i} must start at 0");
        }
    }
}
