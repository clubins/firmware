//! PMS5003 driver stub — gated by the `sensor-pms5003` flag.
//!
//! UART particulate sensor; emits the new `pm25` ingest type — the firmware
//! side of the one-topic-per-type rule; the server receiver is a backend
//! follow-up (spec §Decisions). The UART front-end and the PMS5003 frame
//! parse/checksum arrive with the driver PRs.

use crate::boundary::Measurement;
use crate::core1::{SensorDriver, SensorMeta};

pub static META: SensorMeta = SensorMeta {
    name: "pms5003",
    ingest_type: "pm25",
};

#[derive(Debug)]
pub struct Pms5003Stub {
    /// Completed readings since boot (observable for tests).
    pub readings: u32,
}

impl Pms5003Stub {
    pub const fn new() -> Self {
        Pms5003Stub { readings: 0 }
    }
}

impl Default for Pms5003Stub {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorDriver for Pms5003Stub {
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
    fn meta_maps_pms5003_onto_the_new_pm25_type() {
        assert_eq!(META.name, "pms5003");
        assert_eq!(META.ingest_type, "pm25");
    }

    #[test]
    fn scaffold_poll_reports_no_measurement() {
        let mut p = Pms5003Stub::new();
        assert_eq!(p.poll(), None);
        assert_eq!(p.readings, 1);
    }
}
