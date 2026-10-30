//! BH1750 source node — gated by the `sensor-bh1750` flag (pulls
//! `driver-i2c`).
//!
//! Driver contract (spec §Sensor Expansion): I2C; ambient light in lux →
//! the new `light` measurement type (one-topic-per-type rule; no server
//! receiver yet). The I2C read lands with the driver PR; the scaffold
//! emits a deterministic reading per poll.

use crate::measurement::Measurement;
use crate::nodekit::{self, MeasRing};

/// Default I2C address (ADDR tied low; 0x5D when high).
pub const I2C_ADDRESS: u8 = 0x23;

/// Ingest type name on the wire (new type: `light`).
pub const INGEST_TYPE: &str = "light";

/// The BH1750 node: polled I2C in, measurements out.
pub struct Node {
    /// Completed measurements waiting for a transport drain.
    pub out: MeasRing,
    readings: u64,
    emitted: u64,
}

impl Node {
    pub const fn new() -> Self {
        Self {
            out: MeasRing::new(),
            readings: 0,
            emitted: 0,
        }
    }

    /// Polls (register reads attempted) since boot.
    pub fn readings(&self) -> u64 {
        self.readings
    }

    /// Measurements accepted into the output ring since boot.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Static footprint of this node (state + ring), for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step: read the sensor and emit one `light` measurement.
    /// Returns 1 when a measurement was emitted.
    pub fn poll(&mut self) -> usize {
        self.readings = self.readings.wrapping_add(1);
        let r = self.readings;
        // Scaffold reading: deterministic values; the driver PR replaces
        // this with the real conversion.
        let m = Measurement::Light {
            lux: 100 + (r % 900) as u16,
        };
        if nodekit::offer(&self.out, m) {
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
    fn polls_emit_light_measurements() {
        let mut node = Node::new();
        assert_eq!(node.poll(), 1);
        match node.out.pop() {
            Some(Measurement::Light { lux }) => assert!((100..1000).contains(&lux)),
            other => panic!("expected a light measurement, got {other:?}"),
        }
        assert_eq!(node.readings(), 1);
    }
}
