//! Typed measurement model — the firmware-side counterpart of the ingest
//! contract.
//!
//! Every payload type mirrors a Go struct in the backend's ingest service
//! (`server-infra/ingest/service/*.go` in the source monorepo): field names,
//! units and validation rules are the wire contract, and the HTTP JSON codec
//! ([`crate::transports::http_json`]) encodes and decodes these types 1:1.
//!
//! Numeric semantics follow the service exactly:
//!
//! * measurement values are finite and must fit the server's Float32 columns
//!   (`measurementValue` in service.go) — the model stores `f32`, the
//!   device-native width, so the range check collapses to a finiteness check;
//! * `occupancy_count` is a whole number that fits `UInt32` (`occupancyCount`
//!   in pax.go) — stored as `u32`;
//! * `result` is the closed vocabulary of the `m_status.result` column
//!   (`knownResults` in status.go) — stored as the [`StatusResult`] enum, so
//!   a new value is a firmware change, mirroring the server's "adding a value
//!   is a deploy" friction.
//!
//! The device-side readings ([`EnvironmentalReading`], [`Co2Reading`],
//! [`ParticulateReading`], [`LightReading`]) have no ingest endpoint of their
//! own; they are the sensor outputs the transport layer composes into ingest
//! payloads. Their units follow the backend's conventions (`weather.go`):
//! Celsius, percent, hectopascals, µg/m³, lux and ppm.

use core::fmt;

/// Closed vocabulary of the `m_status.result` column (`knownResults` in the
/// backend's status.go). The reason a thing failed belongs in the status
/// message, not in a new value here.
pub const RESULT_VALUES: [&str; 5] =
    ["started", "stopped", "success", "degraded", "failed"];

/// Why a model value was rejected.
///
/// Device-side validation is deliberately stricter than the server's decode:
/// the server silently mends malformed identifiers (`rowID` regenerates a
/// lost measurement id, `rowTime` falls back to its own clock), which would
/// quietly sever a measurement's correlation or shift its recorded time. The
/// model surfaces those at the source instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    /// `sensor_id` is not a UUID — the server answers 400 (`service.go`).
    SensorIdNotUuid,
    /// `apikey` is empty — the server would answer 401.
    ApiKeyEmpty,
    /// `time` is not RFC 3339 — the server would silently stamp the row with
    /// its own clock (`rowTime` in service.go).
    TimeInvalid,
    /// A `measurement_id`/`status_id` is malformed; the server would silently
    /// regenerate it (`rowID` in service.go).
    IdentifierInvalid,
    /// A measurement value is NaN or infinite.
    NotFinite(&'static str),
    /// `subsystem` is empty — the server answers 400 (`status.go`).
    SubsystemEmpty,
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SensorIdNotUuid => f.write_str("sensor_id must be a UUID"),
            Self::ApiKeyEmpty => f.write_str("apikey must not be empty"),
            Self::TimeInvalid => f.write_str("time must be RFC 3339"),
            Self::IdentifierInvalid => {
                f.write_str("identifier must be a UUID when present")
            }
            Self::NotFinite(name) => write!(f, "{name} must be finite"),
            Self::SubsystemEmpty => f.write_str("subsystem is required"),
        }
    }
}

impl core::error::Error for ModelError {}

/// Whether `s` is a UUID in any form `google/uuid` accepts: canonical
/// 8-4-4-4-12 hex, optionally braced, with an optional `urn:uuid:` prefix, or
/// 32 hex digits without separators. The device only ever emits the canonical
/// form; the wider acceptance mirrors `uuid.Parse` so the decoder can stand
/// in for the server's authorize stage.
pub fn is_uuid(s: &str) -> bool {
    let s = s.strip_prefix("urn:uuid:").unwrap_or(s);
    let s = s.strip_prefix('{').unwrap_or(s);
    let s = s.strip_suffix('}').unwrap_or(s);
    let bytes = s.as_bytes();
    if bytes.len() == 32 {
        return bytes.iter().all(u8::is_ascii_hexdigit);
    }
    if bytes.len() != 36 {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if *b != b'-' {
                    return false;
                }
            }
            _ => {
                if !b.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

/// Whether `s` is a timestamp the server's `rowTime` keeps: RFC 3339 with
/// optional fractional seconds and a `Z` or `±hh:mm` offset. `rowTime`
/// silently falls back to the server clock for anything else, which would
/// shift the measurement's recorded time — the device validates instead.
/// As with Go's `time.Parse`, only upper-case `T` and `Z` are accepted
/// despite the RFC allowing lower case.
pub fn is_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20 {
        return false;
    }
    let Some(year) = digits4(b, 0) else {
        return false;
    };
    let (Some(month), Some(day)) = (digits2(b, 5), digits2(b, 8)) else {
        return false;
    };
    let (Some(hour), Some(minute), Some(second)) =
        (digits2(b, 11), digits2(b, 14), digits2(b, 17))
    else {
        return false;
    };
    if b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return false;
    }

    // Optional fractional seconds: '.' plus one or more digits.
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest = &frac[digits..];
    }

    match rest.as_bytes().first() {
        Some(b'Z') => rest.len() == 1,
        Some(b'+') | Some(b'-') => {
            rest.len() == 6
                && digits2(rest.as_bytes(), 1).is_some_and(|h| h <= 23)
                && rest.as_bytes()[3] == b':'
                && digits2(rest.as_bytes(), 4).is_some_and(|m| m <= 59)
        }
        _ => false,
    }
}

fn digits2(b: &[u8], i: usize) -> Option<u32> {
    let hi = (b[i] as char).to_digit(10)?;
    let lo = (b[i + 1] as char).to_digit(10)?;
    Some(hi * 10 + lo)
}

fn digits4(b: &[u8], i: usize) -> Option<u32> {
    Some(digits2(b, i)? * 100 + digits2(b, i + 2)?)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    let leap =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    }
}

/// The shared request credentials — the flat JSON fields every ingest
/// endpoint carries (`credentials` in service.go, embedded into each payload
/// struct and promoted flat by `encoding/json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Credentials<'a> {
    /// UUID of the reporting sensor.
    pub sensor_id: &'a str,
    /// Ingest API key (NVS-stored on the device).
    pub apikey: &'a str,
    /// Reading time, RFC 3339.
    pub time: &'a str,
}

impl<'a> Credentials<'a> {
    /// Validates in the server's order: `sensor_id` as a UUID (the authorize
    /// stage answers 400), then the fields a malformed value would silently
    /// mend.
    pub fn new(
        sensor_id: &'a str,
        apikey: &'a str,
        time: &'a str,
    ) -> Result<Self, ModelError> {
        if !is_uuid(sensor_id) {
            return Err(ModelError::SensorIdNotUuid);
        }
        if apikey.is_empty() {
            return Err(ModelError::ApiKeyEmpty);
        }
        if !is_rfc3339(time) {
            return Err(ModelError::TimeInvalid);
        }
        Ok(Self {
            sensor_id,
            apikey,
            time,
        })
    }
}

/// An absent id is fine (the server regenerates one); a malformed id is a
/// firmware bug and is reported rather than mended.
fn require_id(id: &str) -> Result<(), ModelError> {
    if id.is_empty() || is_uuid(id) {
        Ok(())
    } else {
        Err(ModelError::IdentifierInvalid)
    }
}

fn require_finite(name: &'static str, value: f32) -> Result<(), ModelError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(ModelError::NotFinite(name))
    }
}

/// One decibel reading — the body posted to `/sensors/decibel`
/// (`decibelPayload`). `dba`/`dbc` are the A- and C-weighted levels in dB SPL
/// per the DSP notes (`decibel.md`); there is no LCpeak field by design —
/// the microphone clips below club peaks, so the ingest contract does not
/// carry one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecibelMeasurement<'a> {
    pub credentials: Credentials<'a>,
    /// Correlation id; the device emits a UUIDv7.
    pub measurement_id: &'a str,
    pub dba: f32,
    pub dbc: f32,
}

impl<'a> DecibelMeasurement<'a> {
    pub fn new(
        credentials: Credentials<'a>,
        measurement_id: &'a str,
        dba: f32,
        dbc: f32,
    ) -> Result<Self, ModelError> {
        require_id(measurement_id)?;
        require_finite("dba", dba)?;
        require_finite("dbc", dbc)?;
        Ok(Self {
            credentials,
            measurement_id,
            dba,
            dbc,
        })
    }
}

/// One occupancy reading — the body posted to `/sensors/pax` (`paxPayload`).
/// Stored as `u32`, the column type; on the wire it is a JSON number the
/// server validates as whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaxMeasurement<'a> {
    pub credentials: Credentials<'a>,
    pub measurement_id: &'a str,
    pub occupancy_count: u32,
}

impl<'a> PaxMeasurement<'a> {
    pub fn new(
        credentials: Credentials<'a>,
        measurement_id: &'a str,
        occupancy_count: u32,
    ) -> Result<Self, ModelError> {
        require_id(measurement_id)?;
        Ok(Self {
            credentials,
            measurement_id,
            occupancy_count,
        })
    }
}

/// The six required weather values, in `m_weather` column order and units
/// (Celsius, percent, millimetres, percent, km/h, hectopascals). Grouped so
/// composition code can pass one value bundle instead of six bare floats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherValues {
    pub temperature: f32,
    pub humidity: f32,
    pub precipitation: f32,
    pub rain_probability: f32,
    pub wind_speed: f32,
    pub pressure: f32,
}

impl WeatherValues {
    fn validate(&self) -> Result<(), ModelError> {
        require_finite("temperature", self.temperature)?;
        require_finite("humidity", self.humidity)?;
        require_finite("precipitation", self.precipitation)?;
        require_finite("rain_probability", self.rain_probability)?;
        require_finite("wind_speed", self.wind_speed)?;
        require_finite("pressure", self.pressure)?;
        Ok(())
    }
}

/// One weather reading — the body posted to `/sensors/weather`
/// (`weatherPayload`). Field names carry no unit suffixes, matching the
/// `m_weather` columns. All six fields are required by the server, so a
/// sensor that cannot fill one (a PIR has no pressure) does not post weather
/// at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherMeasurement<'a> {
    pub credentials: Credentials<'a>,
    pub measurement_id: &'a str,
    pub values: WeatherValues,
}

impl<'a> WeatherMeasurement<'a> {
    pub fn new(
        credentials: Credentials<'a>,
        measurement_id: &'a str,
        values: WeatherValues,
    ) -> Result<Self, ModelError> {
        require_id(measurement_id)?;
        values.validate()?;
        Ok(Self {
            credentials,
            measurement_id,
            values,
        })
    }
}

/// The closed result vocabulary, typed so a firmware-side typo cannot split
/// the server's LowCardinality column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusResult {
    Started,
    Stopped,
    Success,
    Degraded,
    Failed,
}

impl StatusResult {
    /// The wire form — the `m_status.result` column value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
            Self::Success => "success",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
        }
    }

    /// The only wire forms the server accepts (`knownResults`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "started" => Some(Self::Started),
            "stopped" => Some(Self::Stopped),
            "success" => Some(Self::Success),
            "degraded" => Some(Self::Degraded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// A device status report — the body posted to `/sensors/status`
/// (`statusPayload`). `subsystem` is deliberately unvalidated beyond
/// emptiness: sensors name whatever part of themselves they are reporting on,
/// and a new one must not require a deploy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusMeasurement<'a> {
    pub credentials: Credentials<'a>,
    pub status_id: &'a str,
    pub subsystem: &'a str,
    pub result: StatusResult,
    pub message: &'a str,
}

impl<'a> StatusMeasurement<'a> {
    pub fn new(
        credentials: Credentials<'a>,
        status_id: &'a str,
        subsystem: &'a str,
        result: StatusResult,
        message: &'a str,
    ) -> Result<Self, ModelError> {
        require_id(status_id)?;
        if subsystem.is_empty() {
            return Err(ModelError::SubsystemEmpty);
        }
        Ok(Self {
            credentials,
            status_id,
            subsystem,
            result,
            message,
        })
    }
}

/// Environmental trio from the I²C climate sensors (BME280, or SHT4x plus a
/// pressure source). These are the readings a weather post is composed from,
/// when the device knows all six weather fields.
#[derive(Clone, Copy, Debug)]
pub struct EnvironmentalReading {
    pub temperature_c: f32,
    pub humidity_percent: f32,
    pub pressure_hpa: f32,
}

/// Temperature and humidity from the SHT4x — its two measured quantities,
/// with no pressure field: the SHT4x measures none, and the model never
/// fabricates one. A weather post composes pressure from the BME280.
#[derive(Clone, Copy, Debug)]
pub struct TemperatureHumidityReading {
    pub temperature_c: f32,
    pub humidity_percent: f32,
}

/// CO₂ concentration from the SCD4x, in ppm (the sensor's native `u16`).
#[derive(Clone, Copy, Debug)]
pub struct Co2Reading {
    pub co2_ppm: u16,
}

/// Particulate mass concentrations from the PMS5003, in µg/m³.
#[derive(Clone, Copy, Debug)]
pub struct ParticulateReading {
    pub pm1_0_ugm3: u16,
    pub pm2_5_ugm3: u16,
    pub pm10_0_ugm3: u16,
}

/// Ambient light from the BH1750, in lux (raw counts divided by 1.2).
#[derive(Clone, Copy, Debug)]
pub struct LightReading {
    pub lux: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10";
    const T: &str = "2026-10-30T21:15:42Z";

    fn creds() -> Credentials<'static> {
        Credentials::new(SID, "k", T).expect("test credentials are valid")
    }

    #[test]
    fn uuid_accepts_the_forms_go_accepts() {
        assert!(is_uuid(SID));
        assert!(is_uuid(SID.to_ascii_uppercase().as_str()));
        assert!(is_uuid("0199d0e98b7a7cc29fd38f2b6f1e4a10"));
        assert!(is_uuid("{0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10}"));
        assert!(is_uuid("urn:uuid:0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10"));
        assert!(!is_uuid(""));
        assert!(!is_uuid("nope"));
        assert!(!is_uuid("0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a1g"));
        assert!(!is_uuid("0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a1"));
    }

    #[test]
    fn rfc3339_accepts_what_rowtime_keeps() {
        assert!(is_rfc3339("2026-10-30T21:15:42Z"));
        assert!(is_rfc3339("2026-10-30T21:15:42.187Z"));
        assert!(is_rfc3339("2026-10-30T21:15:42.187123456Z"));
        assert!(is_rfc3339("2026-10-30T23:15:42+02:00"));
        assert!(is_rfc3339("2024-02-29T00:00:00Z"));
        assert!(is_rfc3339("2026-10-30T21:15:60Z")); // leap second (RFC 3339 §5.7)
        assert!(!is_rfc3339("2026-10-30T21:15:42"));
        assert!(!is_rfc3339("2026-10-30t21:15:42Z"));
        assert!(!is_rfc3339("2026-10-30T21:15:42z"));
        assert!(!is_rfc3339("2023-02-29T00:00:00Z"));
        assert!(!is_rfc3339("2026-13-01T00:00:00Z"));
        assert!(!is_rfc3339("2026-10-30T24:00:00Z"));
        assert!(!is_rfc3339("2026-10-30T21:15:61Z"));
        assert!(!is_rfc3339("2026-10-30T21:15:42."));
        assert!(!is_rfc3339("2026-10-30 21:15:42Z"));
    }

    #[test]
    fn credentials_reject_bad_fields() {
        assert_eq!(
            Credentials::new("nope", "k", T),
            Err(ModelError::SensorIdNotUuid)
        );
        assert_eq!(Credentials::new(SID, "", T), Err(ModelError::ApiKeyEmpty));
        assert_eq!(
            Credentials::new(SID, "k", "now"),
            Err(ModelError::TimeInvalid)
        );
        let good = Credentials::new(SID, "k", T).expect("valid credentials");
        assert_eq!(good.sensor_id, SID);
    }

    #[test]
    fn decibel_rejects_non_finite_and_bad_ids() {
        assert_eq!(
            DecibelMeasurement::new(creds(), "", f32::NAN, 0.0),
            Err(ModelError::NotFinite("dba"))
        );
        assert_eq!(
            DecibelMeasurement::new(creds(), "", 0.0, f32::INFINITY),
            Err(ModelError::NotFinite("dbc"))
        );
        assert_eq!(
            DecibelMeasurement::new(creds(), "also-not-a-uuid", 0.0, 0.0),
            Err(ModelError::IdentifierInvalid)
        );
        assert!(DecibelMeasurement::new(creds(), "", -72.5, 68.25).is_ok());
    }

    #[test]
    fn weather_requires_all_six_values_finite() {
        let values = |t, h, p, r, w, pr| WeatherValues {
            temperature: t,
            humidity: h,
            precipitation: p,
            rain_probability: r,
            wind_speed: w,
            pressure: pr,
        };
        assert!(
            WeatherMeasurement::new(
                creds(),
                "",
                values(21.5, 55.0, 0.0, 40.0, 12.5, 1013.25)
            )
            .is_ok()
        );
        assert_eq!(
            WeatherMeasurement::new(
                creds(),
                "",
                values(21.5, 55.0, 0.0, 40.0, 12.5, f32::NAN)
            ),
            Err(ModelError::NotFinite("pressure"))
        );
    }

    #[test]
    fn status_requires_subsystem_and_types_the_result() {
        assert_eq!(
            StatusMeasurement::new(creds(), "", "", StatusResult::Success, ""),
            Err(ModelError::SubsystemEmpty)
        );
        assert!(
            StatusMeasurement::new(creds(), "", "dsp", StatusResult::Success, "").is_ok()
        );
        for value in RESULT_VALUES {
            let parsed = StatusResult::parse(value).expect("vocabulary parses");
            assert_eq!(parsed.as_str(), value);
        }
        assert_eq!(StatusResult::parse("ok"), None);
        assert_eq!(StatusResult::parse(""), None);
    }

    #[test]
    fn pax_and_identifiers_round_trip_u32() {
        let m = PaxMeasurement::new(creds(), "", u32::MAX).expect("u32 is always valid");
        assert_eq!(m.occupancy_count, u32::MAX);
        assert!(PaxMeasurement::new(creds(), SID, 0).is_ok());
    }

    #[test]
    fn device_readings_are_plain_data() {
        let environmental = EnvironmentalReading {
            temperature_c: 21.5,
            humidity_percent: 55.0,
            pressure_hpa: 1013.25,
        };
        let co2 = Co2Reading { co2_ppm: 812 };
        let particulate = ParticulateReading {
            pm1_0_ugm3: 4,
            pm2_5_ugm3: 7,
            pm10_0_ugm3: 11,
        };
        let light = LightReading { lux: 212.5 };
        assert!(environmental.temperature_c > co2.co2_ppm as f32 * 0.0);
        assert_eq!(particulate.pm10_0_ugm3, 11);
        assert!(light.lux > 0.0);
    }
}
