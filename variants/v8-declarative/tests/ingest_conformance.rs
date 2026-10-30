#![cfg(feature = "transport-http")]
//! Ingest contract conformance.
//!
//! The fixtures in `tests/fixtures/ingest/` pin the wire format the Go
//! ingest service accepts (`server-infra/ingest/service/*.go`): valid
//! bodies must decode, re-encode byte-exactly (the encoder emits the Go
//! structs' field order, so the fixture bytes are the golden), and round-
//! trip losslessly; every reject fixture must fail with the same error
//! class the service produces for that body. The files are plain JSON so a
//! future backend-side harness can replay them without Rust.

use firmware_v8_declarative::http_json::{
    Credentials, DECIBEL_PATH, DecodeError, IngestEvent, IngestResponse, PAX_PATH,
    STATUS_PATH, WEATHER_PATH, decode, decode_response, encode,
};
use firmware_v8_declarative::measurement::{Measurement, StatusReport, StatusResult};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ingest");

/// Fixture body with the trailing newline the files carry for git-friendliness.
fn fixture(rel: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}/{rel}"))
        .unwrap_or_else(|e| panic!("fixture {rel} readable: {e}"))
        .trim_end_matches('\n')
        .to_string()
}

fn creds(sensor: &str, time: Option<&str>) -> Credentials {
    Credentials {
        sensor_id: sensor.into(),
        apikey: "development-api-key".into(),
        time: time.map(Into::into),
    }
}

const SENSOR_A: &str = "0a20720e-bf44-4f4c-9a59-d40dd827e7f7";
const SENSOR_B: &str = "630d11c4-57d8-4a72-9899-3eb995f6e147";
const ROW_1: &str = "019873f4-1234-7abc-8def-0123456789ab";
const ROW_2: &str = "019873f4-5678-7abc-8def-0123456789ab";
const ROW_3: &str = "019873f4-9abc-7abc-8def-0123456789ab";
const ROW_4: &str = "019873f4-def0-7abc-8def-0123456789ab";
const AT_NOON: &str = "2026-08-21T12:00:00Z";

fn decibel_event() -> IngestEvent {
    IngestEvent {
        credentials: creds(SENSOR_A, Some(AT_NOON)),
        row_id: Some(ROW_1.into()),
        measurement: Measurement::decibel(72.4, 78.1).expect("finite"),
    }
}

fn pax_event() -> IngestEvent {
    IngestEvent {
        credentials: creds(SENSOR_B, Some(AT_NOON)),
        row_id: Some(ROW_2.into()),
        measurement: Measurement::pax(19),
    }
}

fn weather_event(values: (f32, f32, f32, f32, f32, f32)) -> IngestEvent {
    let (temperature, humidity, precipitation, rain_probability, wind_speed, pressure) =
        values;
    IngestEvent {
        credentials: creds(SENSOR_A, Some(AT_NOON)),
        row_id: Some(ROW_3.into()),
        measurement: Measurement::weather(
            temperature,
            humidity,
            precipitation,
            rain_probability,
            wind_speed,
            pressure,
        )
        .expect("finite"),
    }
}

fn status_event_full() -> IngestEvent {
    IngestEvent {
        credentials: creds(SENSOR_B, Some(AT_NOON)),
        row_id: Some(ROW_4.into()),
        measurement: Measurement::status(StatusReport {
            status_id: Some(ROW_4.into()),
            subsystem: "power".into(),
            result: StatusResult::Degraded,
            message: Some("battery 12%".into()),
        }),
    }
}

fn status_event_minimal() -> IngestEvent {
    IngestEvent {
        credentials: creds(SENSOR_B, None),
        row_id: None,
        measurement: Measurement::status(StatusReport {
            status_id: None,
            subsystem: "net".into(),
            result: StatusResult::Started,
            message: None,
        }),
    }
}

/// Every valid fixture decodes to the expected event and re-encodes to the
/// exact bytes on disk.
#[test]
fn valid_fixtures_decode_and_reencode_byte_exact() {
    let cases: Vec<(&str, &'static str, IngestEvent)> = vec![
        ("decibel.json", DECIBEL_PATH, decibel_event()),
        ("pax.json", PAX_PATH, pax_event()),
        (
            "weather.json",
            WEATHER_PATH,
            weather_event((18.4, 71.0, 0.2, 40.0, 12.7, 1018.4)),
        ),
        (
            "weather-exponent.json",
            WEATHER_PATH,
            // wind_speed below the 1e-6 threshold pins the exponent-form
            // spelling, the encoding/json f/e format rule.
            weather_event((18.4, 60.0, 0.0, 10.0, 1.5e-7, 1013.0)),
        ),
        ("status.json", STATUS_PATH, status_event_full()),
        ("status-minimal.json", STATUS_PATH, status_event_minimal()),
    ];
    for (file, path, expected) in cases {
        let body = fixture(file);
        let decoded = decode(path, &body).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(decoded, expected, "{file}");
        let encoded = encode(&decoded).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(encoded.path, path, "{file}");
        assert_eq!(encoded.body, body, "{file}: re-encode must be byte-exact");
    }
}

/// The decoder accepts the public paths (what the device POSTs) and the
/// stripped service paths (what the Go mux registers) identically.
#[test]
fn service_side_paths_decode_identically() {
    let body = fixture("decibel.json");
    let via_public = decode("/v1/sensors/decibel", &body).expect("public path");
    let via_service = decode("/sensors/decibel", &body).expect("service path");
    assert_eq!(via_public, via_service);
}

#[test]
fn unknown_paths_are_rejected() {
    // A credentials-complete body, so the failure is the path, not the
    // credentials (the decoder parses those before it maps the path).
    let body = fixture("pax.json");
    assert_eq!(
        decode("/sensors/co2", &body),
        Err(DecodeError::UnknownPath("/sensors/co2".into()))
    );
}

/// How each reject fixture must fail, mirroring the service: exact error
/// where the message is structural, variant-only for positioned syntax
/// errors whose byte offset varies.
enum Expected {
    Syntax,
    Exact(DecodeError),
}

#[test]
fn reject_fixtures_fail_like_the_service() {
    let table: Vec<(&str, &'static str, Expected)> = vec![
        (
            "decibel-dba-required.json",
            "/sensors/decibel",
            Expected::Exact(DecodeError::MissingField { field: "dba" }),
        ),
        (
            "decibel-dba-not-number.json",
            "/sensors/decibel",
            Expected::Exact(DecodeError::WrongType {
                field: "dba",
                want: "a number",
                got: "a string",
            }),
        ),
        // 1e300 survives JSON but not float32 (the measurementValue rule).
        (
            "decibel-dba-out-of-f32.json",
            "/sensors/decibel",
            Expected::Exact(DecodeError::BadValue {
                field: "dba",
                why: "must be a finite 32-bit float",
            }),
        ),
        // 1e400 does not survive JSON (the encoding/json range error).
        (
            "decibel-dba-out-of-json.json",
            "/sensors/decibel",
            Expected::Exact(DecodeError::BadValue {
                field: "dba",
                why: "number is out of JSON range",
            }),
        ),
        (
            "pax-occupancy-count-required.json",
            "/sensors/pax",
            Expected::Exact(DecodeError::MissingField {
                field: "occupancy_count",
            }),
        ),
        (
            "pax-occupancy-fractional.json",
            "/sensors/pax",
            Expected::Exact(DecodeError::BadValue {
                field: "occupancy_count",
                why: "must be a whole number",
            }),
        ),
        (
            "pax-occupancy-negative.json",
            "/sensors/pax",
            Expected::Exact(DecodeError::BadValue {
                field: "occupancy_count",
                why: "must be between 0 and 4294967295",
            }),
        ),
        (
            "pax-occupancy-over-u32.json",
            "/sensors/pax",
            Expected::Exact(DecodeError::BadValue {
                field: "occupancy_count",
                why: "must be between 0 and 4294967295",
            }),
        ),
        (
            "sensor-id-not-uuid.json",
            "/sensors/pax",
            Expected::Exact(DecodeError::BadValue {
                field: "sensor_id",
                why: "must be a UUID",
            }),
        ),
        (
            "sensor-id-missing.json",
            "/sensors/pax",
            Expected::Exact(DecodeError::MissingField { field: "sensor_id" }),
        ),
        (
            "weather-humidity-required.json",
            "/sensors/weather",
            Expected::Exact(DecodeError::MissingField { field: "humidity" }),
        ),
        // Absent and empty subsystem are the server's one error.
        (
            "status-subsystem-required.json",
            "/sensors/status",
            Expected::Exact(DecodeError::BadValue {
                field: "subsystem",
                why: "is required",
            }),
        ),
        (
            "status-subsystem-empty.json",
            "/sensors/status",
            Expected::Exact(DecodeError::BadValue {
                field: "subsystem",
                why: "is required",
            }),
        ),
        (
            "status-result-unknown.json",
            "/sensors/status",
            Expected::Exact(DecodeError::BadValue {
                field: "result",
                why: "must be one of [started stopped success degraded failed]",
            }),
        ),
        ("invalid-json.json", "/sensors/pax", Expected::Syntax),
        ("trailing-data.json", "/sensors/pax", Expected::Syntax),
    ];
    for (file, path, expected) in table {
        let body = fixture(&format!("reject/{file}"));
        let error = decode(path, &body)
            .err()
            .unwrap_or_else(|| panic!("reject/{file} must not decode"));
        match expected {
            Expected::Syntax => assert!(
                matches!(error, DecodeError::Syntax(_)),
                "{file}: expected a syntax error, got {error:?}"
            ),
            Expected::Exact(expected) => {
                assert_eq!(error, expected, "{file}");
            }
        }
    }
}

/// The legacy v1 firmware posts dropped fields (`dba_min`/`dba_max`);
/// unknown keys must be ignored, exactly as the service does.
#[test]
fn legacy_v1_extra_fields_are_ignored() {
    let body = format!(
        "{{\"sensor_id\":\"{SENSOR_A}\",\"apikey\":\"k\",\"time\":\"{AT_NOON}\",\
         \"dba_min\":60.0,\"dba_max\":85.0,\"dba\":72.4,\"dbc\":78.1}}"
    );
    let event = decode("/sensors/decibel", &body).expect("decodes");
    assert_eq!(
        event.measurement,
        Measurement::decibel(72.4, 78.1).expect("finite")
    );
}

/// The server decodes bodies with absent `apikey` (then 401s) and absent
/// `time` (then stamps now) — the decoder mirrors both fallbacks.
#[test]
fn absent_apikey_and_time_decode_with_server_fallbacks() {
    let body = format!("{{\"sensor_id\":\"{SENSOR_A}\",\"dba\":1,\"dbc\":2}}");
    let event = decode("/sensors/decibel", &body).expect("decodes");
    assert_eq!(event.credentials.apikey, "");
    assert_eq!(event.credentials.time, None);
    assert_eq!(event.row_id, None);
}

#[test]
fn response_fixtures_decode() {
    assert_eq!(
        decode_response(&fixture("response-accepted.json")),
        Ok(IngestResponse::Status("accepted".into()))
    );
    assert_eq!(
        decode_response(&fixture("response-error.json")),
        Ok(IngestResponse::Error("apikey invalid".into()))
    );
}
