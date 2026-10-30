//! PIR occupancy driver stub — gated by the `sensor-pax` flag.
//!
//! Hardware note (spec §Sensor Expansion): no PIR part is documented in the
//! repo; this driver ships behind its flag with a config-chosen GPIO pin
//! convention and reports motion detections honestly — not a head count.
//! The GPIO interrupt front-end (pin, debounce) arrives with the driver
//! PRs; the stub pins the [`SensorDriver`] signature.

use crate::boundary::Measurement;
use crate::core1::{SensorDriver, SensorMeta};

pub static META: SensorMeta = SensorMeta {
    name: "pax",
    ingest_type: "pax",
};

#[derive(Debug)]
pub struct PaxStub {
    /// Motion detections since boot (observable for tests).
    pub detections: u32,
}

impl PaxStub {
    pub const fn new() -> Self {
        PaxStub { detections: 0 }
    }
}

impl Default for PaxStub {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorDriver for PaxStub {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn poll(&mut self) -> Option<Measurement> {
        // GPIO interrupt front-end arrives with the driver PRs; the scaffold
        // reports nothing on device.
        self.detections = self.detections.wrapping_add(1);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_matches_the_one_topic_per_type_contract() {
        assert_eq!(META.name, "pax");
        assert_eq!(META.ingest_type, "pax");
    }

    #[test]
    fn scaffold_poll_reports_no_measurement() {
        let mut p = PaxStub::new();
        assert_eq!(p.poll(), None);
        assert_eq!(p.detections, 1);
    }
}
