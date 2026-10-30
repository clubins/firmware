//! BH1750 driver stub — gated by the `sensor-bh1750` flag.
//!
//! I2C ambient light sensor; emits the new `light` ingest type — the
//! firmware side of the one-topic-per-type rule; the server receiver is a
//! backend follow-up (spec §Decisions). The I2C front-end arrives with the
//! driver PRs.

use crate::boundary::Measurement;
use crate::core1::{SensorDriver, SensorMeta};

pub static META: SensorMeta = SensorMeta {
    name: "bh1750",
    ingest_type: "light",
};

#[derive(Debug)]
pub struct Bh1750Stub {
    /// Completed readings since boot (observable for tests).
    pub readings: u32,
}

impl Bh1750Stub {
    pub const fn new() -> Self {
        Bh1750Stub { readings: 0 }
    }
}

impl Default for Bh1750Stub {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorDriver for Bh1750Stub {
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
    fn meta_maps_bh1750_onto_the_new_light_type() {
        assert_eq!(META.name, "bh1750");
        assert_eq!(META.ingest_type, "light");
    }

    #[test]
    fn scaffold_poll_reports_no_measurement() {
        let mut b = Bh1750Stub::new();
        assert_eq!(b.poll(), None);
        assert_eq!(b.readings, 1);
    }
}
