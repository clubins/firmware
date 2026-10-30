//! Measurement and status event types — the vocabulary tasks exchange.
//!
//! Payload shapes follow the wire contracts, not internal convenience:
//!
//! - Decibel carries the full MQTT v2 `Decibel` message shape
//!   (`api/mqtt/v2/sensor/decibel.proto`): equivalent continuous levels plus
//!   Fast-weighted extrema. The HTTP decibel endpoint posts only `dba`/`dbc`
//!   (=`laeq`/`lceq`) and drops the extrema — that asymmetry lives in the
//!   server ingest service, not in this model.
//! - Weather fields match `server-infra/ingest/service/weather.go`. Every
//!   field is required on the HTTP wire, so `None` marks "this driver does
//!   not measure it" and the transport encoder must gate completeness before
//!   posting.
//! - Status results are the closed vocabulary of `service/status.go`
//!   (`started|stopped|success|degraded|failed`); `subsystem` is open.
//!
//! `time_ms = 0` means "clock not synchronised" (MQTT v2 `Header` semantics),
//! so the server can distinguish *no clock* from *wrong clock*.

use heapless::String as HeapString;

/// One decibel measurement window — the MQTT v2 `Decibel` message shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecibelSample {
    /// Length of the measurement window, in milliseconds.
    pub window_ms: u32,
    /// The input reached the top of the sensor's measurement range at some
    /// point in the window; every level in this sample may then be wrong
    /// (usually too low). From `MeasurementMeta.overload`.
    pub overload: bool,
    /// A-weighted equivalent continuous level (LAeq), dB SPL.
    pub laeq: f32,
    /// Fast-weighted A-weighted minimum within the window (LAFmin), dB SPL.
    pub la_fmin: f32,
    /// Fast-weighted A-weighted maximum within the window (LAFmax), dB SPL.
    pub la_fmax: f32,
    /// C-weighted equivalent continuous level (LCeq), dB SPL.
    pub lceq: f32,
    /// Fast-weighted C-weighted maximum within the window (LCFmax), dB SPL.
    /// There is deliberately no LCpeak: the microphone clips at about
    /// 120 dB SPL, below club peaks (see `decibel.proto`).
    pub lc_fmax: f32,
}

/// One occupancy report. PIR front-ends report detections honestly — this is
/// a motion-detection count, not a head count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaxSample {
    /// Whole number of detections, 0..=u32::MAX (ingest validation bounds).
    pub occupancy_count: u32,
}

/// One environmental reading. Units follow the m_weather columns, which
/// deliberately carry no unit suffixes: °C, %, mm, %, km/h, hPa.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WeatherSample {
    pub temperature: Option<f32>,
    pub humidity: Option<f32>,
    pub precipitation: Option<f32>,
    pub rain_probability: Option<f32>,
    pub wind_speed: Option<f32>,
    pub pressure: Option<f32>,
}

/// One CO₂ reading, in ppm (new ingest type; one-topic-per-type rule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Co2Sample {
    pub ppm: f32,
}

/// One particulate reading, µg/m³ (new ingest type; one-topic-per-type rule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pm25Sample {
    pub ug_m3: f32,
}

/// One ambient-light reading, in lux (new ingest type; one-topic-per-type rule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightSample {
    pub lux: f32,
}

/// The payload of a measurement event — one variant per ingest type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeasurementPayload {
    Decibel(DecibelSample),
    Pax(PaxSample),
    Weather(WeatherSample),
    Co2(Co2Sample),
    Pm25(Pm25Sample),
    Light(LightSample),
}

impl MeasurementPayload {
    /// Ingest type name for one-topic-per-type routing: the HTTP path segment
    /// (`/v1/sensors/<type>`) and the MQTT v2 data topic suffix
    /// (`v2/sensors/<uuid>/data/<type>`). Status is not a measurement type —
    /// it travels separately (see [`StatusEvent`]).
    pub fn ingest_type(&self) -> &'static str {
        match self {
            Self::Decibel(_) => "decibel",
            Self::Pax(_) => "pax",
            Self::Weather(_) => "weather",
            Self::Co2(_) => "co2",
            Self::Pm25(_) => "pm25",
            Self::Light(_) => "light",
        }
    }
}

/// A measurement flowing from a sensor task to the transport task.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasurementEvent {
    pub payload: MeasurementPayload,
    /// Production timestamp, Unix epoch milliseconds UTC; `0` = clock not
    /// synchronised (MQTT v2 `Header.time_ms` semantics). For a decibel
    /// window this is the end of the window.
    pub time_ms: u64,
    /// This topic's message counter within the boot, from the publishing
    /// task's [`session::Sequencer`](crate::session::Sequencer): 0, 1, 2, ….
    /// Together with the boot id it is the server's dedup key (`topics.md`);
    /// the HTTP encoder also folds it into the `measurement_id` it sends.
    pub seq: u32,
}

/// Closed status vocabulary — `knownResults` in `service/status.go`.
/// Adding a value is a server deploy, by design; the reason for a failure
/// belongs in the status message, not in a new result value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusResult {
    Started,
    Stopped,
    Success,
    Degraded,
    Failed,
}

impl StatusResult {
    /// The wire value for HTTP status reports (`service/status.go`).
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

/// A status report from an ops task — the `service/status.go` payload shape
/// plus the header pair every message carries (`topics.md`; the status topic
/// has its own seq counter).
#[derive(Debug, Clone, PartialEq)]
pub struct StatusEvent {
    /// Production timestamp, Unix epoch milliseconds UTC; `0` = clock not
    /// synchronised (MQTT v2 `Header.time_ms` semantics).
    pub time_ms: u64,
    /// The status topic's message counter within the boot, from the
    /// reporting task's [`session::Sequencer`](crate::session::Sequencer).
    pub seq: u32,
    /// What part of the device is reporting (e.g. `lifecycle`, `power`).
    /// Open vocabulary on purpose: a new subsystem must not need a deploy.
    pub subsystem: HeapString<32>,
    pub result: StatusResult,
    pub message: HeapString<128>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_type_names_match_the_wire_contracts() {
        // HTTP path segments: POST /v1/sensors/<type> — decibel.go, pax.go,
        // weather.go register decibel/weather/pax (+status); MQTT v2 data
        // topics carry the same suffix per the one-topic-per-type rule.
        let kinds: [(&MeasurementPayload, &str); 6] = [
            (&MeasurementPayload::Decibel(decibel_sample()), "decibel"),
            (
                &MeasurementPayload::Pax(PaxSample { occupancy_count: 1 }),
                "pax",
            ),
            (
                &MeasurementPayload::Weather(WeatherSample::default()),
                "weather",
            ),
            (&MeasurementPayload::Co2(Co2Sample { ppm: 400.0 }), "co2"),
            (
                &MeasurementPayload::Pm25(Pm25Sample { ug_m3: 12.5 }),
                "pm25",
            ),
            (
                &MeasurementPayload::Light(LightSample { lux: 300.0 }),
                "light",
            ),
        ];
        for (payload, expected) in kinds {
            assert_eq!(payload.ingest_type(), expected);
        }
    }

    fn decibel_sample() -> DecibelSample {
        DecibelSample {
            window_ms: 1000,
            overload: false,
            laeq: 92.5,
            la_fmin: 80.0,
            la_fmax: 104.0,
            lceq: 98.0,
            lc_fmax: 108.5,
        }
    }

    #[test]
    fn decibel_sample_copies_the_v2_proto_field_set() {
        let sample = decibel_sample();
        assert_eq!(sample.window_ms, 1000);
        assert!(!sample.overload);
        assert_eq!(sample.laeq, 92.5);
        assert_eq!(sample.la_fmin, 80.0);
        assert_eq!(sample.la_fmax, 104.0);
        assert_eq!(sample.lceq, 98.0);
        assert_eq!(sample.lc_fmax, 108.5);
    }

    #[test]
    fn status_result_covers_exactly_the_closed_vocabulary() {
        // knownResults = ["started", "stopped", "success", "degraded", "failed"]
        let all = [
            StatusResult::Started,
            StatusResult::Stopped,
            StatusResult::Success,
            StatusResult::Degraded,
            StatusResult::Failed,
        ];
        let wire: [&str; 5] = core::array::from_fn(|i| all[i].as_str());
        assert_eq!(
            wire,
            ["started", "stopped", "success", "degraded", "failed"]
        );
    }

    #[test]
    fn weather_default_marks_every_field_unmeasured() {
        let weather = WeatherSample::default();
        assert!(weather.temperature.is_none());
        assert!(weather.humidity.is_none());
        assert!(weather.precipitation.is_none());
        assert!(weather.rain_probability.is_none());
        assert!(weather.wind_speed.is_none());
        assert!(weather.pressure.is_none());
    }
}
