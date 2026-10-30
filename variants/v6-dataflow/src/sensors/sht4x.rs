//! SHT4x source node — gated by the `sensor-sht4x` flag (pulls
//! `driver-i2c`).
//!
//! Driver contract (spec §Sensor Expansion): I2C; temperature + humidity →
//! `weather` measurements (pressure stays unset — SHT4x has no barometer).
//! Alternative to the BME280's temp/hum: when both are compiled, config
//! picks per-device. The I2C read lands with the driver PR; the scaffold
//! emits a deterministic reading per poll.

use crate::measurement::Measurement;
use crate::nodekit::{self, MeasRing};

/// Default I2C address for the SHT40/SHT41/SHT45 family.
pub const I2C_ADDRESS: u8 = 0x44;

/// Ingest type name on the wire (`weather`, shared with BME280).
pub const INGEST_TYPE: &str = "weather";

/// The SHT4x node: polled I2C in, measurements out.
pub struct Node {
    /// Completed measurements waiting for a transport drain.
    pub out: MeasRing,
    readings: u64,
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

    /// One stage step: read the sensor and emit one `weather` measurement.
    /// Returns 1 when a measurement was emitted.
    pub fn poll(&mut self) -> usize {
        self.readings = self.readings.wrapping_add(1);
        let r = self.readings;
        // Scaffold reading: deterministic values; the driver PR replaces
        // this with the real conversion.
        let m = Measurement::Weather {
            temperature_c: Some(200 + (r % 80) as i16), // 20.0–27.9 °C
            humidity_pct: Some(45 + (r % 15) as u8),    // 45–59 %
            pressure_hpa: None,                         // no barometer on SHT4x
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
    fn polls_emit_weather_without_pressure() {
        let mut node = Node::new();
        assert_eq!(node.poll(), 1);
        // First reading is deterministic: readings increments before the
        // fold, so poll #1 folds r = 1 (201 = 20.1 °C, 46 %).
        assert_eq!(
            node.out.pop(),
            Some(Measurement::Weather {
                temperature_c: Some(201),
                humidity_pct: Some(46),
                pressure_hpa: None,
            })
        );
        assert_eq!(node.readings(), 1);
    }
}
