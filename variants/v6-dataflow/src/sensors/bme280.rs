//! BME280 source node — gated by the `sensor-bme280` flag (pulls
//! `driver-i2c`).
//!
//! Driver contract (spec §Sensor Expansion): I2C; temperature, humidity,
//! pressure → `weather` measurements. The server's weather fields also
//! include precipitation/rain-probability/wind, but those are virtual
//! Open-Meteo fields today — the node leaves them unset. The I2C register
//! read lands with the driver PR; the scaffold emits a deterministic
//! reading per poll.

use crate::measurement::Measurement;
use crate::nodekit::{self, MeasRing};

/// Default I2C address (SDO tied low; 0x77 when high).
pub const I2C_ADDRESS: u8 = 0x76;

/// Ingest type name on the wire (HTTP path `/v1/sensors/weather`, MQTT
/// topic suffix `…/data/weather`).
pub const INGEST_TYPE: &str = "weather";

/// The BME280 node: polled I2C in, measurements out.
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
        // Scaffold reading: deterministic values cycling in plausible
        // ranges; the driver PR replaces this with the real conversion.
        let m = Measurement::Weather {
            temperature_c: Some(180 + (r % 100) as i16), // 18.0–27.9 °C
            humidity_pct: Some(40 + (r % 20) as u8),     // 40–59 %
            pressure_hpa: Some(1013),
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
    fn polls_emit_weather_measurements() {
        let mut node = Node::new();
        assert_eq!(node.poll(), 1);
        assert_eq!(node.poll(), 1);
        assert_eq!(node.readings(), 2);
        assert_eq!(node.emitted(), 2);
        for _ in 0..2 {
            match node.out.pop() {
                Some(Measurement::Weather {
                    temperature_c: Some(t),
                    humidity_pct: Some(h),
                    pressure_hpa: Some(p),
                }) => {
                    assert!((180..280).contains(&t));
                    assert!((40..60).contains(&h));
                    assert_eq!(p, 1013);
                }
                other => panic!("expected a weather measurement, got {other:?}"),
            }
        }
    }

    #[test]
    fn no_out_rings_overflow_under_normal_cadence() {
        // One emission per poll and the transport drains every step, so a
        // burst of polls still stays under the ring capacity × drains.
        let mut node = Node::new();
        for _ in 0..nodekit::MEAS_RING_CAP {
            node.poll();
        }
        assert_eq!(node.out.rejected(), 0);
    }
}
