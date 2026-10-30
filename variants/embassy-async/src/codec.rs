//! HTTP JSON codec — request bodies for the ingest service, encoded to the
//! exact Go payload structs (`server-infra/ingest/service/*.go`).
//!
//! The Go structs embed `credentials`, so every body is flat: `sensor_id`,
//! `apikey`, `time`, then the endpoint's own fields, in struct order.
//! `encoding/json` skips unknown keys, but the firmware sends exactly the
//! struct's keys and nothing else — a byte-exact fixture per endpoint pins
//! the bodies (see `tests/ingest_conformance.rs`).
//!
//! Values follow the server's validation:
//!
//! - Floats must be finite — `measurementValue` rejects NaN/Inf/above
//!   float32 range with a 400, so this codec refuses to encode them at all.
//!   Levels leave as core's shortest round-trip decimal, so the f64 the
//!   server parses is the f32 that was measured (ClickHouse stores Float32).
//! - `occupancy_count` must be whole and within 0..=u32::MAX — the model
//!   field is a u32, so encoding it as a JSON integer is always valid.
//! - All six weather fields are required pointers server-side, so an
//!   incomplete sample is refused rather than zero-filled: a missing
//!   reading must never become a genuine measurement of zero.
//! - `time` is RFC3339. An unsynchronised clock (`time_ms == 0`, the MQTT v2
//!   no-clock marker) sends an empty string, which the server's `rowTime`
//!   treats as "stamp ingestion time" — the HTTP path has no 0 marker to
//!   preserve.
//!
//! Payload types with no HTTP endpoint (co2, pm25, light — MQTT v2
//! one-topic-per-type designs) are refused: encoding a body for an endpoint
//! that does not exist would invent a contract.

use core::fmt::Write as _;
use heapless::String as HeapString;

use crate::events::{MeasurementEvent, MeasurementPayload, StatusEvent};
use crate::session::{BootId, derive_message_id};

/// Upper bound for an encoded body. The server caps bodies at 1 MiB; the
/// largest weather body is ~300 bytes, and the headroom absorbs core's
/// worst-case float rendering (39 digits for a huge f32, six of them in a
/// weather body).
pub const JSON_BODY_MAX: usize = 768;

/// An encoded request body, built in place by the transport task.
pub type JsonBody = HeapString<JSON_BODY_MAX>;

/// The ingest path for a measurement payload —
/// `POST https://ingest.clubins.de/v1/sensors/<type>` (Caddy strips the
/// `/v1` prefix before the Go service sees it). `None` for the payload
/// types that only exist on MQTT v2 so far.
#[must_use]
pub const fn http_endpoint(payload: &MeasurementPayload) -> Option<&'static str> {
    match payload {
        MeasurementPayload::Decibel(_) => Some("/v1/sensors/decibel"),
        MeasurementPayload::Pax(_) => Some("/v1/sensors/pax"),
        MeasurementPayload::Weather(_) => Some("/v1/sensors/weather"),
        MeasurementPayload::Co2(_)
        | MeasurementPayload::Pm25(_)
        | MeasurementPayload::Light(_) => None,
    }
}

/// The ingest path for status reports (`service/status.go` registers
/// `POST /sensors/status`; any sensor may report on itself).
pub const STATUS_ENDPOINT: &str = "/v1/sensors/status";

/// The credentials block every ingest body carries flat (`service.go`
/// `credentials`): `sensor_id` as a UUID string, `apikey` plaintext, and
/// the production `time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Credentials<'a> {
    pub sensor_id: &'a str,
    pub apikey: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// A float field was NaN or infinite — the server would 400 it
    /// (`measurementValue`). Names the offending field.
    NotFinite(&'static str),
    /// A weather sample is missing a required field. Names the first
    /// missing field in the server's fixed validation order.
    WeatherIncomplete(&'static str),
    /// The payload type has no HTTP ingest endpoint (MQTT-v2-only design).
    NoHttpEndpoint,
    /// The body buffer filled — an internal bound, not the server's 1 MiB.
    BufferFull,
}

/// Encode a measurement into its endpoint's exact JSON body, overwriting
/// `out`. `boot_id` + `event.seq` fold into the `measurement_id` (see
/// [`crate::session::derive_message_id`]).
///
/// On error the buffer contents are unspecified — drop them.
pub fn encode_measurement(
    credentials: &Credentials,
    boot_id: &BootId,
    event: &MeasurementEvent,
    out: &mut JsonBody,
) -> Result<(), EncodeError> {
    // Validate the payload kind up front so a refusal leaves the caller's
    // buffer logically untouched.
    if http_endpoint(&event.payload).is_none() {
        return Err(EncodeError::NoHttpEndpoint);
    }

    out.clear();
    write_open(out, credentials, event.time_ms)?;
    push(out, ",\"measurement_id\":")?;
    push_json_string(out, derive_message_id(boot_id, event.seq).as_str())?;
    match event.payload {
        // The HTTP decibel endpoint takes only the equivalent continuous
        // levels: dba = LAeq, dbc = LCeq. The Fast extrema and window/
        // overload metadata are MQTT v2's richer shape and are dropped
        // here, exactly as the server contract prescribes.
        MeasurementPayload::Decibel(sample) => {
            push(out, ",\"dba\":")?;
            push_f32(out, "dba", sample.laeq)?;
            push(out, ",\"dbc\":")?;
            push_f32(out, "dbc", sample.lceq)?;
        }
        MeasurementPayload::Pax(sample) => {
            push(out, ",\"occupancy_count\":")?;
            write!(out, "{}", sample.occupancy_count)
                .map_err(|_| EncodeError::BufferFull)?;
        }
        MeasurementPayload::Weather(sample) => {
            // In the server's fixed validation order (weather.go).
            push(out, ",\"temperature\":")?;
            push_f32(
                out,
                "temperature",
                sample
                    .temperature
                    .ok_or(EncodeError::WeatherIncomplete("temperature"))?,
            )?;
            push(out, ",\"humidity\":")?;
            push_f32(
                out,
                "humidity",
                sample
                    .humidity
                    .ok_or(EncodeError::WeatherIncomplete("humidity"))?,
            )?;
            push(out, ",\"precipitation\":")?;
            push_f32(
                out,
                "precipitation",
                sample
                    .precipitation
                    .ok_or(EncodeError::WeatherIncomplete("precipitation"))?,
            )?;
            push(out, ",\"rain_probability\":")?;
            push_f32(
                out,
                "rain_probability",
                sample
                    .rain_probability
                    .ok_or(EncodeError::WeatherIncomplete("rain_probability"))?,
            )?;
            push(out, ",\"wind_speed\":")?;
            push_f32(
                out,
                "wind_speed",
                sample
                    .wind_speed
                    .ok_or(EncodeError::WeatherIncomplete("wind_speed"))?,
            )?;
            push(out, ",\"pressure\":")?;
            push_f32(
                out,
                "pressure",
                sample
                    .pressure
                    .ok_or(EncodeError::WeatherIncomplete("pressure"))?,
            )?;
        }
        // The up-front check already refused these; the arm exists so the
        // match stays exhaustive without a wildcard.
        MeasurementPayload::Co2(_)
        | MeasurementPayload::Pm25(_)
        | MeasurementPayload::Light(_) => {
            unreachable!("endpointless payloads are refused before encoding")
        }
    }
    push(out, "}")
}

/// Encode a status report into the `POST /v1/sensors/status` body, over
/// writing `out`. `boot_id` + `event.seq` fold into the `status_id`.
///
/// On error the buffer contents are unspecified — drop them.
pub fn encode_status(
    credentials: &Credentials,
    boot_id: &BootId,
    event: &StatusEvent,
    out: &mut JsonBody,
) -> Result<(), EncodeError> {
    out.clear();
    write_open(out, credentials, event.time_ms)?;
    push(out, ",\"status_id\":")?;
    push_json_string(out, derive_message_id(boot_id, event.seq).as_str())?;
    push(out, ",\"subsystem\":")?;
    push_json_string(out, event.subsystem.as_str())?;
    push(out, ",\"result\":")?;
    push_json_string(out, event.result.as_str())?;
    push(out, ",\"message\":")?;
    push_json_string(out, event.message.as_str())?;
    push(out, "}")
}

/// The flat credentials prefix every body opens with, in struct order.
fn write_open(
    out: &mut JsonBody,
    credentials: &Credentials,
    time_ms: u64,
) -> Result<(), EncodeError> {
    push(out, "{\"sensor_id\":")?;
    push_json_string(out, credentials.sensor_id)?;
    push(out, ",\"apikey\":")?;
    push_json_string(out, credentials.apikey)?;
    push(out, ",\"time\":")?;
    push_time(out, time_ms)
}

/// The `time` field: RFC3339 with millisecond precision (ClickHouse is
/// DateTime64(3)), or an empty string when the clock is not synchronised.
fn push_time(out: &mut JsonBody, time_ms: u64) -> Result<(), EncodeError> {
    if time_ms == 0 {
        // The HTTP contract has no no-clock marker: `rowTime` falls back to
        // now() for anything unparseable, which is the closest honest value
        // an ingest-time POST can offer.
        return push_json_string(out, "");
    }
    push(out, "\"")?;
    format_rfc3339_ms(time_ms, out)?;
    push(out, "\"")
}

/// UTC timestamp for Unix epoch milliseconds, RFC3339 `YYYY-MM-DDTHH:MM:SS.mmmZ`.
/// Years beyond four digits print wider (`{:04}` is a minimum, not a cap).
fn format_rfc3339_ms(time_ms: u64, out: &mut JsonBody) -> Result<(), EncodeError> {
    let secs = time_ms / 1000;
    let millis = (time_ms % 1000) as u32;
    let days = (secs / 86_400) as i64;
    let secs_of_day = (secs % 86_400) as u32;

    // Howard Hinnant's civil_from_days: days since 1970-01-01 → (y, m, d).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    write!(
        out,
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y, m, d, hour, minute, second, millis
    )
    .map_err(|_| EncodeError::BufferFull)
}

/// A JSON string literal with the mandatory escapes; anything above 0x1f
/// passes through as UTF-8.
fn push_json_string(out: &mut JsonBody, value: &str) -> Result<(), EncodeError> {
    push(out, "\"")?;
    for ch in value.chars() {
        match ch {
            '"' => push(out, "\\\"")?,
            '\\' => push(out, "\\\\")?,
            '\n' => push(out, "\\n")?,
            '\r' => push(out, "\\r")?,
            '\t' => push(out, "\\t")?,
            '\u{08}' => push(out, "\\b")?,
            '\u{0c}' => push(out, "\\f")?,
            c if (c as u32) < 0x20 => {
                push(out, "\\u")?;
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let value = c as u32;
                for shift in [12, 8, 4, 0] {
                    let nibble = ((value >> shift) & 0xf) as usize;
                    out.push(HEX[nibble] as char)
                        .map_err(|_| EncodeError::BufferFull)?;
                }
            }
            c => out.push(c).map_err(|_| EncodeError::BufferFull)?,
        }
    }
    push(out, "\"")
}

/// A JSON number for an f32 field. Refuses non-finite values the server
/// would 400; renders core's shortest round-trip decimal, so the server
/// reads back exactly the measured f32.
fn push_f32(
    out: &mut JsonBody,
    field: &'static str,
    value: f32,
) -> Result<(), EncodeError> {
    if !value.is_finite() {
        return Err(EncodeError::NotFinite(field));
    }
    write!(out, "{}", value).map_err(|_| EncodeError::BufferFull)
}

fn push(out: &mut JsonBody, text: &str) -> Result<(), EncodeError> {
    out.push_str(text).map_err(|_| EncodeError::BufferFull)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        Co2Sample, DecibelSample, LightSample, PaxSample, Pm25Sample, StatusResult,
        WeatherSample,
    };

    const SENSOR_ID: &str = "9c3f1e42-1111-2222-3333-444455556666";
    const APIKEY: &str = "ck_7f3a2b";

    fn credentials() -> Credentials<'static> {
        Credentials {
            sensor_id: SENSOR_ID,
            apikey: APIKEY,
        }
    }

    fn boot() -> BootId {
        BootId::new([
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10,
        ])
    }

    const T_2026_EVENING_MS: u64 = 1_793_394_929_123; // 2026-10-30T21:15:29.123Z

    fn decibel_event(seq: u32, time_ms: u64, laeq: f32, lceq: f32) -> MeasurementEvent {
        MeasurementEvent {
            payload: MeasurementPayload::Decibel(DecibelSample {
                window_ms: 1000,
                overload: false,
                laeq,
                la_fmin: laeq - 10.0,
                la_fmax: laeq + 10.0,
                lceq,
                lc_fmax: lceq + 10.0,
            }),
            time_ms,
            seq,
        }
    }

    #[test]
    fn decibel_body_carries_exactly_the_go_struct_fields() {
        let mut body = JsonBody::new();
        encode_measurement(
            &credentials(),
            &boot(),
            &decibel_event(0, T_2026_EVENING_MS, 92.5, 98.0),
            &mut body,
        )
        .unwrap();
        assert_eq!(
            body.as_str(),
            "{\"sensor_id\":\"9c3f1e42-1111-2222-3333-444455556666\",\"apikey\":\"ck_7f3a2b\",\
             \"time\":\"2026-10-30T21:15:29.123Z\",\
             \"measurement_id\":\"01020304-0506-8708-890a-0b0c00000000\",\
             \"dba\":92.5,\"dbc\":98}"
        );
        // The Fast extrema and window metadata are deliberately absent: the
        // HTTP endpoint has no columns for them (decibel.go).
        assert!(!body.contains("la_fmin"));
        assert!(!body.contains("window_ms"));
    }

    #[test]
    fn pax_body_encodes_the_count_as_a_whole_json_integer() {
        let mut body = JsonBody::new();
        encode_measurement(
            &credentials(),
            &boot(),
            &MeasurementEvent {
                payload: MeasurementPayload::Pax(PaxSample {
                    occupancy_count: 42,
                }),
                time_ms: T_2026_EVENING_MS,
                seq: 7,
            },
            &mut body,
        )
        .unwrap();
        assert!(body.contains("\"occupancy_count\":42"));
        // measurement_id folds the seq tail: …0b0c00000007.
        assert!(
            body.contains("\"measurement_id\":\"01020304-0506-8708-890a-0b0c00000007\"")
        );
    }

    #[test]
    fn weather_body_carries_all_six_fields_in_server_order() {
        let mut body = JsonBody::new();
        encode_measurement(
            &credentials(),
            &boot(),
            &MeasurementEvent {
                payload: MeasurementPayload::Weather(WeatherSample {
                    temperature: Some(21.5),
                    humidity: Some(54.25),
                    precipitation: Some(0.5),
                    rain_probability: Some(0.35),
                    wind_speed: Some(12.25),
                    pressure: Some(1013.25),
                }),
                time_ms: T_2026_EVENING_MS,
                seq: 1,
            },
            &mut body,
        )
        .unwrap();
        let order: [&str; 6] = [
            "temperature",
            "humidity",
            "precipitation",
            "rain_probability",
            "wind_speed",
            "pressure",
        ];
        let mut last = 0;
        for key in order {
            let at = body.find(key).unwrap_or_else(|| panic!("{key} missing"));
            assert!(at > last, "{key} out of server order");
            last = at;
        }
        assert!(body.contains("\"temperature\":21.5"));
        assert!(body.contains("\"humidity\":54.25"));
        assert!(body.contains("\"pressure\":1013.25"));
    }

    #[test]
    fn incomplete_weather_is_refused_not_zero_filled() {
        let mut body = JsonBody::new();
        let mut sample = WeatherSample {
            temperature: Some(21.5),
            ..WeatherSample::default()
        };
        sample.humidity = Some(40.0);
        let error = encode_measurement(
            &credentials(),
            &boot(),
            &MeasurementEvent {
                payload: MeasurementPayload::Weather(sample),
                time_ms: T_2026_EVENING_MS,
                seq: 0,
            },
            &mut body,
        )
        .unwrap_err();
        assert_eq!(error, EncodeError::WeatherIncomplete("precipitation"));
    }

    #[test]
    fn non_finite_levels_are_refused() {
        let mut body = JsonBody::new();
        for (laeq, lceq) in [
            (f32::NAN, 98.0),
            (92.5, f32::INFINITY),
            (92.5, f32::NEG_INFINITY),
        ] {
            let error = encode_measurement(
                &credentials(),
                &boot(),
                &decibel_event(0, T_2026_EVENING_MS, laeq, lceq),
                &mut body,
            )
            .unwrap_err();
            assert!(matches!(error, EncodeError::NotFinite("dba" | "dbc")));
        }
    }

    #[test]
    fn payload_types_without_http_endpoints_are_refused() {
        let mut body = JsonBody::new();
        for payload in [
            MeasurementPayload::Co2(Co2Sample { ppm: 400.0 }),
            MeasurementPayload::Pm25(Pm25Sample { ug_m3: 12.5 }),
            MeasurementPayload::Light(LightSample { lux: 300.0 }),
        ] {
            assert_eq!(http_endpoint(&payload), None);
            assert_eq!(
                encode_measurement(
                    &credentials(),
                    &boot(),
                    &MeasurementEvent {
                        payload,
                        time_ms: T_2026_EVENING_MS,
                        seq: 0,
                    },
                    &mut body,
                )
                .unwrap_err(),
                EncodeError::NoHttpEndpoint
            );
            // A refusal must not leave half a body behind.
            assert!(body.is_empty());
        }
    }

    #[test]
    fn status_body_carries_exactly_the_status_struct_fields() {
        let mut body = JsonBody::new();
        let mut event = StatusEvent {
            time_ms: T_2026_EVENING_MS,
            seq: 0,
            subsystem: HeapString::new(),
            result: StatusResult::Started,
            message: HeapString::new(),
        };
        event.subsystem.push_str("lifecycle").unwrap();
        event.message.push_str("boot: v3 event bus").unwrap();
        encode_status(&credentials(), &boot(), &event, &mut body).unwrap();
        assert_eq!(
            body.as_str(),
            "{\"sensor_id\":\"9c3f1e42-1111-2222-3333-444455556666\",\"apikey\":\"ck_7f3a2b\",\
             \"time\":\"2026-10-30T21:15:29.123Z\",\
             \"status_id\":\"01020304-0506-8708-890a-0b0c00000000\",\
             \"subsystem\":\"lifecycle\",\"result\":\"started\",\"message\":\"boot: v3 event bus\"}"
        );
    }

    #[test]
    fn strings_are_escaped_as_json_requires() {
        let mut body = JsonBody::new();
        let mut event = StatusEvent {
            time_ms: T_2026_EVENING_MS,
            seq: 0,
            subsystem: HeapString::new(),
            result: StatusResult::Degraded,
            message: HeapString::new(),
        };
        event.subsystem.push_str("up\"stream\\").unwrap();
        event
            .message
            .push_str("line1\nline2\ttabctrl:\x07")
            .unwrap();
        encode_status(&credentials(), &boot(), &event, &mut body).unwrap();
        assert_eq!(
            body.as_str(),
            "{\"sensor_id\":\"9c3f1e42-1111-2222-3333-444455556666\",\"apikey\":\"ck_7f3a2b\",\
             \"time\":\"2026-10-30T21:15:29.123Z\",\
             \"status_id\":\"01020304-0506-8708-890a-0b0c00000000\",\
             \"subsystem\":\"up\\\"stream\\\\\",\"result\":\"degraded\",\
             \"message\":\"line1\\nline2\\ttabctrl:\\u0007\"}"
        );
    }

    #[test]
    fn unsynchronised_clock_sends_an_empty_time() {
        let mut body = JsonBody::new();
        encode_measurement(
            &credentials(),
            &boot(),
            &decibel_event(0, 0, 92.5, 98.0),
            &mut body,
        )
        .unwrap();
        assert!(body.contains("\"time\":\"\""));
    }

    #[test]
    fn rfc3339_vectors() {
        let mut out = JsonBody::new();
        for (time_ms, expected) in [
            (1_000_000_000_000, "\"2001-09-09T01:46:40.000Z\""),
            (1_709_251_199_999, "\"2024-02-29T23:59:59.999Z\""), // leap day
            (253_402_300_799_999, "\"9999-12-31T23:59:59.999Z\""),
        ] {
            out.clear();
            push_time(&mut out, time_ms).unwrap();
            assert_eq!(out.as_str(), expected);
        }
    }

    #[test]
    fn heapless_fmt_write_errors_on_overflow() {
        // Every numeric field is written through fmt::Write; pin that an
        // overflow surfaces as Err (not a panic), which is what makes
        // BufferFull a real signal rather than a crash.
        let mut scratch = HeapString::<4>::new();
        assert!(core::fmt::Write::write_str(&mut scratch, "01234").is_err());
    }
}
