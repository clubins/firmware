//! PIR occupancy front-end — GPIO interrupt events, deferred through the
//! lock-free ring.
//!
//! No PIR hardware is documented in the repo; the driver ships behind its flag
//! with a config-chosen pin convention. It reports PIR detections honestly —
//! motion events, not a head count.
//!
//! The V4 shape: the GPIO edge ISR pushes [`Motion`] events into an
//! `SpscRing`; the sensor phase drains the ring and buckets detections into
//! the reporting interval. Stub: the GPIO ISR wiring lands with the pax task.

use crate::bus::Bus;
use crate::sensor::{IngestType, Sample, Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "pir",
    ingest_type: IngestType::Pax,
    report_period_ticks: 30_000, // 30 s occupancy reports at a 1 kHz grid
};

/// One motion detection from the ISR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// Absolute grid tick of the detection.
    pub tick: u64,
}

pub struct Pir;

impl Pir {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for Pir {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Pir {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn init(&mut self, _bus: &mut Bus) -> Result<(), SensorError> {
        // Stub: pin setup (from config) + edge interrupt registration land
        // with the pax task; the ISR pushes Motion into the shared ring.
        Ok(())
    }

    fn sample(&mut self, _bus: &mut Bus) -> Result<Option<Sample>, SensorError> {
        // Stub: drain would count Motion events in this reporting window.
        Ok(None)
    }
}
