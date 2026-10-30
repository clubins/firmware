//! PAX (occupancy) source node — gated by the `sensor-pax` flag (pulls
//! `driver-gpio`).
//!
//! Driver contract (spec §Sensor Expansion): GPIO-interrupt PIR front-end;
//! the pin comes from config. In V6 the GPIO ISR is a producer like any DMA
//! engine: it stamps detections into [`Node`]'s ring via [`Node::on_detection`]
//! (non-blocking, losses counted by the ring); [`Node::poll`] folds the
//! pending stamps into one `pax` measurement (`occupancy_count`). It
//! reports PIR detections honestly — not a head-count metric. No PIR
//! hardware is documented in the repo; the pin convention is config-chosen.

use crate::measurement::Measurement;
use crate::nodekit::{self, MeasRing};
use crate::ring::Spsc;

/// Ingest type name on the wire (HTTP path `/v1/sensors/pax`, MQTT topic
/// suffix `…/data/pax`).
pub const INGEST_TYPE: &str = "pax";

/// GPIO event-stamp ring capacity (power of two).
pub const EVENT_RING_CAP: usize = 64;

/// The PAX node: detection stamps in (ISR-fed), measurements out.
pub struct Node {
    /// Detection stamps — pushed by the ISR (the producer), drained by
    /// [`Node::poll`] (the only consumer).
    pub events: Spsc<u64, EVENT_RING_CAP>,
    /// Completed measurements waiting for a transport drain.
    pub out: MeasRing,
    detections: u64,
    emitted: u64,
}

impl Default for Node {
    fn default() -> Self {
        Self::new()
    }
}

impl Node {
    pub const fn new() -> Self {
        Self {
            events: Spsc::new(),
            out: MeasRing::new(),
            detections: 0,
            emitted: 0,
        }
    }

    /// ISR-side producer: stamp one detection. Never blocks; a full ring
    /// drops the stamp (counted in `events.rejected()`) — an ISR cannot
    /// wait, and the loss is visible.
    pub fn on_detection(&self, stamp: u64) -> bool {
        self.events.push(stamp).is_ok()
    }

    /// Detections folded since boot.
    pub fn detections(&self) -> u64 {
        self.detections
    }

    /// Measurements accepted into the output ring since boot.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Static footprint of this node (rings + state), for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step: fold every pending stamp into one `pax` measurement.
    /// Returns 1 when a measurement was emitted.
    pub fn poll(&mut self) -> usize {
        let mut count = 0u8;
        while self.events.pop().is_some() {
            self.detections += 1;
            count = count.saturating_add(1);
        }
        if count == 0 {
            return 0;
        }
        if nodekit::offer(
            &self.out,
            Measurement::Pax {
                occupancy_count: count,
            },
        ) {
            self.emitted += 1;
            1
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detections_fold_into_a_pax_measurement() {
        let mut node = Node::new();
        for stamp in 0..3 {
            assert!(node.on_detection(stamp));
        }
        assert_eq!(node.poll(), 1);
        assert_eq!(node.detections(), 3);
        assert_eq!(
            node.out.pop(),
            Some(Measurement::Pax { occupancy_count: 3 })
        );
        // No detections → no measurement.
        assert_eq!(node.poll(), 0);
    }

    #[test]
    fn full_event_ring_drops_stamps_visibly() {
        let mut node = Node::new();
        for stamp in 0..EVENT_RING_CAP as u64 {
            assert!(node.on_detection(stamp));
        }
        // An ISR cannot wait: the next stamps are dropped and counted.
        assert!(!node.on_detection(u64::MAX - 1));
        assert!(!node.on_detection(u64::MAX));
        assert_eq!(node.events.rejected(), 2);
        // The folded count reports exactly what landed.
        assert_eq!(node.poll(), 1);
        assert_eq!(
            node.out.pop(),
            Some(Measurement::Pax {
                occupancy_count: EVENT_RING_CAP as u8,
            })
        );
    }
}
