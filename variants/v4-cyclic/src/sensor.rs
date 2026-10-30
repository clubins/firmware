//! Sensor driver interface — the contract every driver and the registry share.
//!
//! Drivers are bus-fed and polled by the sensor phase; nothing here blocks or
//! allocates. Sample types carry the exact wire fields their ingest type
//! expects (see `measurement.rs` and the Go ingest structs).

use crate::bus::Bus;

/// Which ingest type a sensor feeds. One topic per type — the server's
/// one-topic-per-type rule (api/mqtt/topics.md) — so the mapping is explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IngestType {
    /// Sound levels: `dba`, `dbc`.
    Decibel,
    /// Occupancy: `occupancy_count`.
    Pax,
    /// Environment: `temperature`, `humidity`, `pressure`.
    Weather,
    /// CO₂ in ppm — new type; server receiver not built yet.
    Co2,
    /// PM2.5 in µg/m³ — new type; server receiver not built yet.
    Pm25,
    /// Ambient light in lux — new type; server receiver not built yet.
    Light,
}

impl IngestType {
    /// The type's own topic/endpoint segment (one topic per type).
    pub const fn segment(self) -> &'static str {
        match self {
            Self::Decibel => "decibel",
            Self::Pax => "pax",
            Self::Weather => "weather",
            Self::Co2 => "co2",
            Self::Pm25 => "pm25",
            Self::Light => "light",
        }
    }
}

/// Static description used by the registry and the schedule builder.
pub struct SensorMeta {
    /// Driver name, e.g. "bme280".
    pub name: &'static str,
    pub ingest_type: IngestType,
    /// Reporting cadence in grid ticks; drives the timer wheel at boot.
    pub report_period_ticks: u32,
}

/// One reading, shaped exactly as its ingest type is transmitted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sample {
    /// Full decibel window per `decibel.md` and `v2/sensor/decibel.proto`.
    Decibel {
        dba: f32,
        dbc: f32,
        la_fmin: f32,
        la_fmax: f32,
        lc_fmax: f32,
        window_ms: u32,
        overload: bool,
    },
    /// PIR detections, reported honestly as motion events — not a head count.
    Pax {
        occupancy_count: u32,
    },
    Weather {
        temperature: f32,
        humidity: f32,
        /// SHT4x has no pressure sensor; BME280 does.
        pressure: Option<f32>,
    },
    Co2 {
        ppm: u16,
    },
    Pm25 {
        ugm3: u16,
    },
    Light {
        lux: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorError {
    /// The bus transaction failed.
    Bus,
    /// The device is not ready to sample (still measuring, or not initialized).
    NotReady,
}

/// The driver contract: init once, then poll from the sensor phase.
pub trait Sensor {
    fn meta(&self) -> &'static SensorMeta;

    fn init(&mut self, bus: &mut Bus) -> Result<(), SensorError>;

    /// One poll. `Ok(None)` = no new reading this time — normal for stubs,
    /// conversion-locked devices, and quiet motion sensors.
    fn sample(&mut self, bus: &mut Bus) -> Result<Option<Sample>, SensorError>;
}
