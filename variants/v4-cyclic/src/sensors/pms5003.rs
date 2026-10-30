//! PMS5003 over UART — PM2.5 particulates.
//!
//! Emits the new `pm25` ingest type per the one-topic-per-type rule. The
//! sensor streams 32-byte frames; the V4 shape has the UART RX ISR frame
//! bytes into the lock-free ring and the sensor phase reassembles them.
//!
//! Stub: the frame parser (start bytes 0x42 0x4D, checksum) and UART setup
//! land with the driver task.

use crate::bus::Bus;
use crate::sensor::{IngestType, Sample, Sensor, SensorError, SensorMeta};

pub const META: SensorMeta = SensorMeta {
    name: "pms5003",
    ingest_type: IngestType::Pm25,
    report_period_ticks: 30_000,
};

pub struct Pms5003;

impl Pms5003 {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for Pms5003 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensor for Pms5003 {
    fn meta(&self) -> &'static SensorMeta {
        &META
    }

    fn init(&mut self, _bus: &mut Bus) -> Result<(), SensorError> {
        // Stub: UART 9600 8N1 + RX interrupt wiring land with the driver.
        Ok(())
    }

    fn sample(&mut self, _bus: &mut Bus) -> Result<Option<Sample>, SensorError> {
        Ok(None) // Stub: frame reassembly lands with the driver task.
    }
}
