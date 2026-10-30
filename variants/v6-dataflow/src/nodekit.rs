//! Shared machinery for the flag-gated graph nodes.
//!
//! Every node is statically allocated: its rings are `Spsc` values inline
//! in the node struct, its counters are plain integers (nodes are only ever
//! driven from one context — the graph pump — except for the ISR/producer
//! entry points, which use the lock-free rings). The scaffold's node shape:
//!
//! - **Source nodes** (`sensors/*`): own their input ring (bytes from a
//!   DMA engine or stamps from an ISR) or are polled directly (I2C), and
//!   push [`Measurement`]s into their output ring.
//! - **Drain nodes** (`transports/*`): own an input measurement ring and
//!   drain it — the wire write lands with the driver PR.
//! - **Ops nodes** (`ops/*`): status heartbeats ride the routing path like
//!   a sensor; the rest are wired but inert until their driver PRs.

use crate::measurement::Measurement;
use crate::ring::Spsc;

/// Measurement-ring capacity used by every node's output/input ring
/// (power of two). Sized for the reporting cadences in the spec's config
/// file (one measurement per type per interval, transports drain faster).
pub const MEAS_RING_CAP: usize = 64;

/// The measurement ring type shared by all nodes.
pub type MeasRing = Spsc<Measurement, MEAS_RING_CAP>;

/// Push a measurement into a ring without blocking. On a full ring the
/// measurement is dropped; the ring's `rejected` counter records it (the
/// producer never blocks). Returns whether it landed.
pub fn offer(ring: &MeasRing, m: Measurement) -> bool {
    ring.push(m).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_reports_and_counts_landings() {
        let ring: MeasRing = MeasRing::new();
        for i in 0..MEAS_RING_CAP {
            assert!(offer(&ring, Measurement::Co2 { ppm: i as u16 }));
        }
        // The next one overflows: rejected by the ring, reported as false.
        assert!(!offer(&ring, Measurement::Co2 { ppm: 0 }));
        assert_eq!(ring.rejected(), 1);
        assert_eq!(ring.len(), MEAS_RING_CAP);
    }
}
