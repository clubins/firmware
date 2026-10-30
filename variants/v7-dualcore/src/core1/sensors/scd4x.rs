//! SCD4x driver stub — gated by the `sensor-scd4x` flag.
//!
//! I2C CO₂ sensor; emits the new `co2` ingest type — the firmware side of
//! the one-topic-per-type rule (api/mqtt/topics.md); the server receiver is
//! a backend follow-up (spec §Decisions). The I2C front-end arrives with
//! the driver PRs.

use crate::boundary::Measurement;
use crate::core1::{SensorDriver, SensorMeta};

pub static META: SensorMeta = SensorMeta {
    name: "scd4x",
    ingest_type: "co2",
};

#[derive(Debug)]
pub struct Scd4xStub {
    /// Completed readings since boot (observable for tests).
    pub readings: u32,
}

impl Scd4xStub {
    pub const fn new() -> Self {
        Scd4xStub { readings: 0 }
    }
}

impl Default for Scd4xStub {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorDriver for Scd4xStub {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn poll(&mut self) -> Option<Measurement> {
        self.readings = self.readings.wrapping_add(1);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_maps_scd4x_onto_the_new_co2_type() {
        assert_eq!(META.name, "scd4x");
        assert_eq!(META.ingest_type, "co2");
    }

    #[test]
    fn scaffold_poll_reports_no_measurement() {
        let mut s = Scd4xStub::new();
        assert_eq!(s.poll(), None);
        assert_eq!(s.readings, 1);
    }
}
