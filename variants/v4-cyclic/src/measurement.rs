//! The wire measurement model — field-for-field with the Go ingest structs.
//!
//! HTTP JSON bodies serialize from these variants; the field names below are
//! copied from `server-infra/ingest/service/*.go` and are contract-exact.
//! Encoder implementations land with the transport tasks; this module fixes
//! the shapes they must emit.

/// The `credentials` block embedded flat in every HTTP body
/// (`service/service.go` L46–52). The API key lives only in NVS at runtime —
/// never in committed config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Credentials {
    /// Sensor UUID — the server's lookup key.
    pub sensor_id: [u8; 16],
    /// Per-sensor API key.
    pub apikey: ApiKey,
    /// RFC 3339 UTC timestamp. `None` = let the server stamp `now()`.
    pub time: Option<Timestamp>,
}

/// API key bytes as carried in NVS; compared plaintext server-side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApiKey(pub [u8; 32]);

/// RFC 3339 UTC timestamp, rendered by the encoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timestamp(pub u64);

/// One wire message. Field names mirror the Go payload structs exactly:
/// `measurement_id`, `dba`, `dbc`, `occupancy_count`, weather fields,
/// `status_id`, `subsystem`, `result`, `message`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Measurement {
    /// `decibel.go`: MeasurementID, DBA, DBC (required numbers).
    Decibel {
        measurement_id: [u8; 16],
        dba: f32,
        dbc: f32,
    },
    /// `pax.go`: MeasurementID, OccupancyCount (whole, 0..=2³²−1).
    Pax {
        measurement_id: [u8; 16],
        occupancy_count: u32,
    },
    /// `weather.go`: MeasurementID + six float fields; the firmware supplies
    /// temperature/humidity/pressure — precipitation, rain_probability and
    /// wind_speed are virtual Open-Meteo fields the device never reports.
    Weather {
        measurement_id: [u8; 16],
        temperature: f32,
        humidity: f32,
        pressure: Option<f32>,
    },
    /// New ingest types per the one-topic-per-type rule. The firmware emits
    /// them today; the server grows receivers later.
    Co2 {
        measurement_id: [u8; 16],
        ppm: u16,
    },
    Pm25 {
        measurement_id: [u8; 16],
        ugm3: u16,
    },
    Light {
        measurement_id: [u8; 16],
        lux: f32,
    },
    /// `status.go`: StatusID, Subsystem, Result, Message.
    Status {
        status_id: [u8; 16],
        subsystem: Subsystem,
        result: StatusResult,
        message: Message,
    },
}

/// Status subsystem — open vocabulary server-side; these are the in-repo
/// examples (`power`, `battery`, `connection`) plus the firmware itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subsystem {
    Firmware,
    Power,
    Battery,
    Connection,
}

impl Subsystem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Firmware => "firmware",
            Self::Power => "power",
            Self::Battery => "battery",
            Self::Connection => "connection",
        }
    }
}

/// Status `result` — a CLOSED set; the server 400s anything else
/// (`status.go` L18: started|stopped|success|degraded|failed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusResult {
    Started,
    Stopped,
    Success,
    Degraded,
    Failed,
}

impl StatusResult {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
            Self::Success => "success",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
        }
    }
}

/// Free-form status text, bounded like the ring buffers it travels through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message(pub [u8; 64]);
