//! Ingest contract conformance: fixtures pin the wire behavior in both
//! directions.
//!
//! `tests/fixtures/ingest/` holds three kinds of truth, mirroring what the
//! backend's `server-infra/ingest/service` accepts and answers:
//!
//! * `valid/` — canonical request bodies. The tests re-encode a model value
//!   and require byte equality with the file, so the encoder cannot drift
//!   from the contract, then decode the file back to the expected fields.
//! * `reject/` — bodies the service answers with 400. The tests require the
//!   matching [`DecodeError`], in the handler's own check order.
//! * `response/` — the service's exact error bodies. `writeJSON` wraps the
//!   message as `{"error":"…"}\n` (`json.Encoder` appends the newline); the
//!   tests require our `Display` to reproduce the message verbatim, so a
//!   rejected body reads the same on both sides of the contract.
//!
//! The not-JSON and wrong-type cases have no response fixture: the service
//! embeds Go's own `json` detail there (`invalid request body: …`,
//! service.go:161), which we intentionally do not restate — our Display
//! carries the fixed prefix only.

use firmware_v5_hsm::measurement::{
    Credentials, DecibelMeasurement, PaxMeasurement, StatusMeasurement, StatusResult,
    WeatherMeasurement, WeatherValues,
};
use firmware_v5_hsm::transports::http_json::{
    DecodeError, decode_decibel, decode_pax, decode_status, decode_weather,
    encode_decibel, encode_pax, encode_status, encode_weather,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ingest");

const SID: &str = "0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10";
const MID: &str = "0199d0e9-8b7c-7d4e-9f5a-2b3c4d5e6f70";
const KEY: &str = "ck_live_01JNE8X4Q2R7T9WY3Z6A5B8CD";
const T: &str = "2026-10-30T21:15:42.187Z";

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}/{rel}"))
        .unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn creds() -> Credentials<'static> {
    Credentials::new(SID, KEY, T).expect("fixture credentials are valid")
}

fn encode_into(bytes: &mut [u8]) -> &str {
    core::str::from_utf8(bytes).expect("the encoder emits UTF-8")
}

// -- valid fixtures: golden encoder output and decoded fields ---------------

#[test]
fn decibel_fixture_is_the_encoder_golden() {
    let body = fixture("valid/decibel_request.json");
    let m = DecibelMeasurement::new(creds(), MID, 72.5, 68.25).expect("valid");
    let mut buf = [0u8; 1024];
    let n = encode_decibel(&m, &mut buf).expect("fits");
    assert_eq!(
        &buf[..n],
        body.as_bytes(),
        "encoder must reproduce the golden body byte for byte"
    );
    let decoded = decode_decibel(&body).expect("the golden body decodes");
    assert_eq!(decoded.credentials.sensor_id.as_ref(), SID);
    assert_eq!(decoded.credentials.apikey.as_ref(), KEY);
    assert_eq!(decoded.credentials.time.as_ref(), T);
    assert_eq!(decoded.measurement_id.as_ref(), MID);
    assert_eq!(decoded.dba, 72.5);
    assert_eq!(decoded.dbc, 68.25);
}

#[test]
fn pax_fixture_is_the_encoder_golden() {
    let body = fixture("valid/pax_request.json");
    let m = PaxMeasurement::new(creds(), MID, 12).expect("valid");
    let mut buf = [0u8; 1024];
    let n = encode_pax(&m, &mut buf).expect("fits");
    assert_eq!(&buf[..n], body.as_bytes());
    let decoded = decode_pax(&body).expect("the golden body decodes");
    assert_eq!(decoded.credentials.sensor_id.as_ref(), SID);
    assert_eq!(decoded.credentials.apikey.as_ref(), KEY);
    assert_eq!(decoded.credentials.time.as_ref(), T);
    assert_eq!(decoded.measurement_id.as_ref(), MID);
    assert_eq!(decoded.occupancy_count, 12);
}

#[test]
fn weather_fixture_is_the_encoder_golden() {
    let body = fixture("valid/weather_request.json");
    let m = WeatherMeasurement::new(
        creds(),
        MID,
        WeatherValues {
            temperature: -4.5,
            humidity: 55.25,
            precipitation: 0.25,
            rain_probability: 40.0,
            wind_speed: 12.5,
            pressure: 1013.25,
        },
    )
    .expect("valid");
    let mut buf = [0u8; 1024];
    let n = encode_weather(&m, &mut buf).expect("fits");
    assert_eq!(&buf[..n], body.as_bytes());
    let decoded = decode_weather(&body).expect("the golden body decodes");
    assert_eq!(decoded.credentials.sensor_id.as_ref(), SID);
    assert_eq!(decoded.measurement_id.as_ref(), MID);
    assert_eq!(decoded.temperature, -4.5);
    assert_eq!(decoded.humidity, 55.25);
    assert_eq!(decoded.precipitation, 0.25);
    assert_eq!(decoded.rain_probability, 40.0);
    assert_eq!(decoded.wind_speed, 12.5);
    assert_eq!(decoded.pressure, 1013.25);
}

#[test]
fn status_fixture_is_the_encoder_golden() {
    let body = fixture("valid/status_request.json");
    let m = StatusMeasurement::new(
        creds(),
        MID,
        "dsp",
        StatusResult::Degraded,
        "upstream timed out",
    )
    .expect("valid");
    let mut buf = [0u8; 1024];
    let n = encode_status(&m, &mut buf).expect("fits");
    assert_eq!(&buf[..n], body.as_bytes());
    let decoded = decode_status(&body).expect("the golden body decodes");
    assert_eq!(decoded.credentials.sensor_id.as_ref(), SID);
    assert_eq!(decoded.status_id.as_ref(), MID);
    assert_eq!(decoded.subsystem.as_ref(), "dsp");
    assert_eq!(decoded.result, StatusResult::Degraded);
    assert_eq!(decoded.message.as_ref(), "upstream timed out");
}

#[test]
fn decibel_fixture_with_extra_keys_still_decodes() {
    let body = fixture("valid/decibel_request_extra_keys.json");
    let decoded = decode_decibel(&body).expect("unknown keys of any shape are skipped");
    assert_eq!(decoded.credentials.sensor_id.as_ref(), SID);
    assert_eq!(decoded.measurement_id.as_ref(), MID);
    assert_eq!(decoded.dba, 72.5);
    assert_eq!(decoded.dbc, 68.25);
}

// -- reject fixtures: the same bodies the Go service answers 400 ------------

#[test]
fn decibel_reject_fixtures_fail_like_the_service() {
    assert_eq!(
        decode_decibel(&fixture("reject/decibel_not_json.json")),
        Err(DecodeError::Syntax)
    );
    assert_eq!(
        decode_decibel(&fixture("reject/sensor_id_not_uuid.json")),
        Err(DecodeError::SensorIdNotUuid)
    );
    assert_eq!(
        decode_decibel(&fixture("reject/decibel_missing_dba.json")),
        Err(DecodeError::Required("dba"))
    );
    assert_eq!(
        decode_decibel(&fixture("reject/decibel_wrong_type_dba.json")),
        Err(DecodeError::WrongType("dba"))
    );
    assert_eq!(
        decode_decibel(&fixture("reject/decibel_out_of_range_dba.json")),
        Err(DecodeError::NotFinite("dba"))
    );
}

#[test]
fn pax_reject_fixtures_fail_like_the_service() {
    assert_eq!(
        decode_pax(&fixture("reject/pax_missing_count.json")),
        Err(DecodeError::Required("occupancy_count"))
    );
    assert_eq!(
        decode_pax(&fixture("reject/pax_fractional_count.json")),
        Err(DecodeError::OccupancyNotWhole)
    );
    assert_eq!(
        decode_pax(&fixture("reject/pax_negative_count.json")),
        Err(DecodeError::OccupancyRange)
    );
}

#[test]
fn status_and_weather_reject_fixtures_fail_like_the_service() {
    assert_eq!(
        decode_status(&fixture("reject/status_missing_subsystem.json")),
        Err(DecodeError::SubsystemEmpty)
    );
    assert_eq!(
        decode_status(&fixture("reject/status_invalid_result.json")),
        Err(DecodeError::ResultInvalid)
    );
    assert_eq!(
        decode_weather(&fixture("reject/weather_missing_pressure.json")),
        Err(DecodeError::Required("pressure"))
    );
}

// -- response fixtures: our diagnosis is the service's response body --------

/// Routes a reject fixture to its endpoint's decoder, discarding the payload:
/// the response fixtures pair with reject fixtures by filename.
fn route_decode(name: &str, body: &str) -> Result<(), DecodeError> {
    if name.starts_with("pax") {
        decode_pax(body).map(|_| ())
    } else if name.starts_with("status") {
        decode_status(body).map(|_| ())
    } else if name.starts_with("weather") {
        decode_weather(body).map(|_| ())
    } else {
        decode_decibel(body).map(|_| ())
    }
}

#[test]
fn response_fixtures_match_the_service_error_bodies() {
    for name in [
        "sensor_id_not_uuid.json",
        "decibel_missing_dba.json",
        "decibel_out_of_range_dba.json",
        "pax_missing_count.json",
        "pax_fractional_count.json",
        "pax_negative_count.json",
        "status_missing_subsystem.json",
        "status_invalid_result.json",
        "weather_missing_pressure.json",
    ] {
        let err = route_decode(name, &fixture(&format!("reject/{name}")))
            .expect_err("the paired reject fixture must be rejected");
        // writeJSON (service.go:234) answers 400 with
        // {"error":"<message>"}\n — the newline comes from json.Encoder.
        let expected = format!("{{\"error\":\"{err}\"}}\n");
        assert_eq!(
            fixture(&format!("response/{name}")),
            expected,
            "response/{name} must quote the service's own words"
        );
    }
}

// -- encoder buffer sizing: the caller's contract ----------------------------

#[test]
fn a_1kib_buffer_holds_any_golden_body() {
    let decibel = DecibelMeasurement::new(creds(), MID, 72.5, 68.25).expect("valid");
    let pax = PaxMeasurement::new(creds(), MID, 12).expect("valid");
    let weather = WeatherMeasurement::new(
        creds(),
        MID,
        WeatherValues {
            temperature: -4.5,
            humidity: 55.25,
            precipitation: 0.25,
            rain_probability: 40.0,
            wind_speed: 12.5,
            pressure: 1013.25,
        },
    )
    .expect("valid");
    let status =
        StatusMeasurement::new(creds(), MID, "dsp", StatusResult::Degraded, "msg")
            .expect("valid");
    for (name, encoded) in [
        ("decibel", encode_decibel(&decibel, &mut [0u8; 1024])),
        ("pax", encode_pax(&pax, &mut [0u8; 1024])),
        ("weather", encode_weather(&weather, &mut [0u8; 1024])),
        ("status", encode_status(&status, &mut [0u8; 1024])),
    ] {
        assert!(encoded.is_ok(), "{name} must fit in 1 KiB");
    }
}

#[test]
fn round_trip_through_the_golden_fixture_is_stable() {
    // Encode → decode → re-encode must be idempotent: the second encoding
    // equals the golden fixture again.
    let body = fixture("valid/pax_request.json");
    let decoded = decode_pax(&body).expect("decodes");
    let rebuilt = PaxMeasurement::new(
        Credentials::new(
            decoded.credentials.sensor_id.as_ref(),
            decoded.credentials.apikey.as_ref(),
            decoded.credentials.time.as_ref(),
        )
        .expect("credentials round-trip"),
        decoded.measurement_id.as_ref(),
        decoded.occupancy_count,
    )
    .expect("valid");
    let mut buf = [0u8; 1024];
    let n = encode_pax(&rebuilt, &mut buf).expect("fits");
    assert_eq!(encode_into(&mut buf[..n]), body.as_str());
}
