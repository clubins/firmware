//! The sensor contract every driver implements.

use crate::measurement::Measurement;

/// Static description of a sensor: what it is, what it emits, in what unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorMeta {
    /// Hardware identity, e.g. "SPK0641HT4H-1" or "BME280".
    pub name: &'static str,
    /// The ingest type this sensor feeds (`Measurement::ingest_type`).
    pub ingest_type: &'static str,
    /// Measurement unit, e.g. "dB SPL", "lx", "µg/m³".
    pub unit: &'static str,
}

/// A driver fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorError {
    /// The sensor did not answer or answered nonsense.
    Fault(&'static str),
    /// The sensor was already initialised.
    AlreadyInitialised,
    /// Sampled before a successful init.
    NotInitialised,
}

/// A sensor driver.
///
/// Buses are owned by the concrete driver (constructed with its bus handles);
/// the trait stays object-safe so the registry can hold every compiled driver
/// behind one pointer.
pub trait Sensor {
    fn metadata(&self) -> &'static SensorMeta;

    /// Bring the sensor up. Called once at bring-up, before any sample.
    fn init(&mut self) -> Result<(), SensorError>;

    /// Take one reading, or `None` when nothing new is due (no motion, no
    /// finished measurement window, ...).
    fn sample(&mut self) -> Option<Measurement>;
}
