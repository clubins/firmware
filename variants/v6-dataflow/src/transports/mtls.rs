//! mTLS variant drain node — gated by the `transport-mtls` flag (pulls
//! `transport-mqtt`).
//!
//! Driver contract (spec §Flag Matrix): client certificates from NVS; the
//! client cert CN carries the sensor UUID. It is the MQTT drain plus TLS
//! identity material — same graph role, same topics. The TLS handshake and
//! key handling land with the network PR; the scaffold keeps the same
//! drain behavior and records that identity material is required.

use crate::nodekit::MeasRing;

/// Ingest type name.
pub const TRANSPORT_NAME: &str = "mqtt-mtls";

/// The mTLS drain node: the MQTT drain with client-identity plumbing.
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

    /// One stage step: drain the input ring. The network PR's TLS session
    /// replaces the count. Returns measurements drained this step.
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
    fn drains_like_the_mqtt_drain() {
        let mut node = Node::new();
        for i in 0..4u16 {
            assert!(node.in_ring.push(Measurement::Light { lux: i }).is_ok());
        }
        assert_eq!(node.poll(), 4);
        assert_eq!(node.sent(), 4);
        assert!(node.in_ring.is_empty());
    }
}
