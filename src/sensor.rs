//! Sensor identity: driver families and their ingest types.
//!
//! The `Sensor` trait and driver objects arrive with the first real driver
//! PR. The scaffold locks the identity half every other PR builds on: which
//! sensor kinds exist, and which ingest measurement type each one feeds.

/// Every sensor driver family the firmware can compile in, one per
/// `sensor-*` feature flag (spec §Sensor Expansion).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SensorKind {
    /// SPK0641HT4H-1 PDM microphone (A/C-weighted level statistics).
    Decibel,
    /// PIR occupancy front-end — reports motion detections honestly, not a
    /// head count.
    Pax,
    /// BME280: temperature, humidity, pressure.
    Bme280,
    /// SHT4x: temperature, humidity.
    Sht4x,
    /// SCD4x: CO2.
    Scd4x,
    /// PMS5003: PM2.5 particulates.
    Pms5003,
    /// BH1750: ambient light.
    Bh1750,
}

impl SensorKind {
    /// All kinds in stable registry order.
    pub const ALL: [SensorKind; 7] = [
        SensorKind::Decibel,
        SensorKind::Pax,
        SensorKind::Bme280,
        SensorKind::Sht4x,
        SensorKind::Scd4x,
        SensorKind::Pms5003,
        SensorKind::Bh1750,
    ];

    /// The ingest measurement type this kind feeds. Both contracts keep one
    /// topic per measurement type; `weather` is shared by the temperature/
    /// humidity/pressure drivers (spec §Sensor Expansion).
    pub const fn ingest_type(self) -> &'static str {
        match self {
            SensorKind::Decibel => "decibel",
            SensorKind::Pax => "pax",
            SensorKind::Bme280 | SensorKind::Sht4x => "weather",
            SensorKind::Scd4x => "co2",
            SensorKind::Pms5003 => "pm25",
            SensorKind::Bh1750 => "light",
        }
    }
}

/// Static description of a compiled-in sensor driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SensorMeta {
    pub kind: SensorKind,
    /// Ingest measurement type (one topic per type on the wire).
    pub ingest_type: &'static str,
}

impl SensorMeta {
    pub const fn new(kind: SensorKind) -> Self {
        SensorMeta {
            ingest_type: kind.ingest_type(),
            kind,
        }
    }
}
