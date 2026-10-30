//! The measurement model every driver emits and every transport encodes.
//!
//! One variant per measurement type, mirroring the server's one-topic-per-type
//! rule (`api/mqtt/topics.md`) and the HTTP ingest endpoints
//! (`server-infra/ingest/service/*.go`). The ingest type names are the wire
//! names: the HTTP path segment (`POST /v1/sensors/<type>`) and the MQTT v2
//! topic's `<data_type>`.

use alloc::boxed::Box;
use heapless::String;

/// The result vocabulary of a status report.
///
/// This is the server's closed set (`service/status.go` `knownResults`): a
/// dashboard filter column nothing polices, so adding a value is a deploy,
/// not a firmware string. The reason for a failure belongs in
/// `StatusReport::message`, not in a new variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusResult {
    Started,
    Stopped,
    Success,
    Degraded,
    Failed,
}

impl StatusResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            StatusResult::Started => "started",
            StatusResult::Stopped => "stopped",
            StatusResult::Success => "success",
            StatusResult::Degraded => "degraded",
            StatusResult::Failed => "failed",
        }
    }
}

/// A status report body. `subsystem` is deliberately open vocabulary
/// (`power`, `battery`, `connection`, `lifecycle`, ...) so naming a new part
/// of the device never needs a server deploy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub subsystem: String<32>,
    pub result: StatusResult,
    /// Large enough for the worst-case boot message: the compiled feature
    /// list (15 flags, ~210 chars).
    pub message: String<256>,
}

/// One reading from a sensor, or a status report from the device itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Measurement {
    /// Sound level (decibel type): A- and C-weighted equivalent continuous
    /// levels, dB SPL.
    Decibel { dba: f32, dbc: f32 },
    /// Occupancy (pax type): PIR detections, not a head count.
    Pax { occupancy_count: u32 },
    /// Environmental conditions (weather type).
    ///
    /// Temperature (°C), humidity (%) and pressure (hPa) come from the
    /// compiled land sensors. Precipitation, rain probability and wind speed
    /// have no firmware source today (they are virtual Open-Meteo fields
    /// server-side). The HTTP weather endpoint requires all six fields
    /// (`service/weather.go`), so how unset fields serialise is the HTTP
    /// transport encoder's decision.
    Weather {
        temperature: f32,
        humidity: f32,
        pressure: f32,
        precipitation: Option<f32>,
        rain_probability: Option<f32>,
        wind_speed: Option<f32>,
    },
    /// CO2 in ppm (co2 type — no server receiver yet; emitted per the
    /// one-topic-per-type rule, see the spec's known-gaps note).
    Co2 { ppm: u16 },
    /// Particulate matter in µg/m³ (pm25 type — no server receiver yet).
    Pm25 { concentration_ugm3: f32 },
    /// Ambient light in lx (light type — no server receiver yet).
    Light { lux: f32 },
    /// Device self-report (status type).
    // Boxed: status reports are rare (heartbeat cadence) and large (the
    // boot report carries the compiled feature list). Value-sizing every
    // enum variant to the status payload would inflate every buffered
    // measurement slot by ~280 bytes; the boxed rare variant keeps offline
    // buffers small. alloc is part of this crate's contract.
    Status(Box<StatusReport>),
}

impl Measurement {
    /// The ingest type name for this measurement: the HTTP path segment
    /// (`POST /v1/sensors/<type>`) and the MQTT v2 `<data_type>`.
    pub fn ingest_type(&self) -> &'static str {
        match self {
            Measurement::Decibel { .. } => "decibel",
            Measurement::Pax { .. } => "pax",
            Measurement::Weather { .. } => "weather",
            Measurement::Co2 { .. } => "co2",
            Measurement::Pm25 { .. } => "pm25",
            Measurement::Light { .. } => "light",
            Measurement::Status(_) => "status",
        }
    }
}

impl From<StatusReport> for Measurement {
    fn from(report: StatusReport) -> Self {
        Measurement::Status(Box::new(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(result: StatusResult) -> Measurement {
        Measurement::Status(Box::new(StatusReport {
            subsystem: String::try_from("lifecycle").unwrap(),
            result,
            message: String::new(),
        }))
    }

    #[test]
    fn ingest_type_names_match_the_server_contract() {
        // The HTTP server mounts exactly these four (`service.go`).
        assert_eq!(
            Measurement::Decibel { dba: 0.0, dbc: 0.0 }.ingest_type(),
            "decibel"
        );
        assert_eq!(Measurement::Pax { occupancy_count: 0 }.ingest_type(), "pax");
        assert_eq!(
            Measurement::Weather {
                temperature: 0.0,
                humidity: 0.0,
                pressure: 0.0,
                precipitation: None,
                rain_probability: None,
                wind_speed: None,
            }
            .ingest_type(),
            "weather"
        );
        assert_eq!(report(StatusResult::Started).ingest_type(), "status");
    }

    #[test]
    fn new_types_follow_one_topic_per_type() {
        // co2, pm25 and light have no server receiver yet; the firmware still
        // emits them under their own type names (spec: known gaps).
        assert_eq!(Measurement::Co2 { ppm: 400 }.ingest_type(), "co2");
        assert_eq!(
            Measurement::Pm25 {
                concentration_ugm3: 12.5
            }
            .ingest_type(),
            "pm25"
        );
        assert_eq!(Measurement::Light { lux: 10.0 }.ingest_type(), "light");
    }

    #[test]
    fn status_result_is_the_server_closed_vocabulary() {
        // service/status.go knownResults, character for character.
        assert_eq!(StatusResult::Started.as_str(), "started");
        assert_eq!(StatusResult::Stopped.as_str(), "stopped");
        assert_eq!(StatusResult::Success.as_str(), "success");
        assert_eq!(StatusResult::Degraded.as_str(), "degraded");
        assert_eq!(StatusResult::Failed.as_str(), "failed");
    }
}
