//! Ingest contract conformance for the HTTP JSON codec.
//!
//! The fixtures under `tests/fixtures/ingest/` are anchored to the ingest
//! service's own Go tests and handler source (`server-infra/ingest`):
//! valid bodies must decode into the device measurement model, rejected
//! bodies must produce the documented decode error, and the response
//! fixtures pin the exact bytes the service answers with — `writeJSON`
//! renders `{"status":...}` for 2xx and `{"error":...}` otherwise, and
//! `json.Encoder.Encode` appends a trailing newline to every response.

#![cfg(feature = "transport-http")]

use firmware_v6_dataflow::measurement::{Measurement, RESULT_FAILED, RESULT_STARTED};
use firmware_v6_dataflow::transports::http_json::{
    Credentials, DecodeError, MAX_ENCODED_BODY_LEN, Post, decode, encode,
};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ingest/");

fn fixture(path: &str) -> Vec<u8> {
    std::fs::read(format!("{DIR}{path}")).expect("fixture file")
}

fn decode_fixture(
    path: &str,
    ty: &'static str,
) -> Result<firmware_v6_dataflow::transports::http_json::Decoded<'static>, DecodeError> {
    let body = fixture(path);
    decode(Box::leak(body.into_boxed_slice()), ty)
}

// ----- valid requests decode into the device model -------------------

#[test]
fn the_server_decibel_body_round_trips_to_canonical_bytes() {
    let decoded =
        decode_fixture("valid/decibel_server_body.json", "decibel").expect("decode");
    assert_eq!(
        decoded.measurement,
        Measurement::Decibel { dba: 724, dbc: 781 }
    );
    assert_eq!(decoded.sensor_id, "0a20720e-bf44-4f4c-9a59-d40dd827e7f7");
    assert_eq!(decoded.id, Some("019873f4-1234-7abc-8def-0123456789ab"));

    // Re-encoding the decoded body must produce the canonical compact
    // form byte for byte.
    let post = Post::new(
        Credentials {
            sensor_id: decoded.sensor_id,
            apikey: decoded.apikey,
        },
        decoded.time,
        decoded.id,
    );
    let mut buf = [0u8; MAX_ENCODED_BODY_LEN];
    let written = encode(&mut buf, &post, &decoded.measurement).expect("encode");
    let golden = fixture("valid/decibel_encoded.json");
    assert_eq!(&buf[..written], &golden[..]);
}

#[test]
fn the_server_pax_body_decodes_to_a_count() {
    let decoded = decode_fixture("valid/pax_server_body.json", "pax").expect("decode");
    assert_eq!(
        decoded.measurement,
        Measurement::Pax {
            occupancy_count: 19
        }
    );
    assert_eq!(decoded.sensor_id, "34bf9ca4-9d08-4a0b-be48-c1e453341db4");
}

#[test]
fn a_zero_pax_count_is_valid() {
    let decoded = decode_fixture("valid/pax_occupancy_zero.json", "pax").expect("decode");
    assert_eq!(decoded.measurement, Measurement::Pax { occupancy_count: 0 });
}

#[test]
fn the_server_weather_body_decodes_to_the_device_fields() {
    // The server body carries the six required values; the device model
    // captures the three it measures and ignores the Open-Meteo virtual
    // fields it never fills.
    let decoded =
        decode_fixture("valid/weather_server_body.json", "weather").expect("decode");
    assert_eq!(
        decoded.measurement,
        Measurement::Weather {
            temperature_c: Some(184),
            humidity_pct: Some(71),
            pressure_hpa: Some(1018), // 1018.4 rounds to 1018
        }
    );
}

#[test]
fn weather_bodies_without_the_virtual_fields_decode() {
    let decoded =
        decode_fixture("valid/weather_device_fields.json", "weather").expect("decode");
    assert_eq!(
        decoded.measurement,
        Measurement::Weather {
            temperature_c: Some(184),
            humidity_pct: Some(71),
            pressure_hpa: Some(1018),
        }
    );
    let decoded = decode_fixture("valid/weather_negative_temperature.json", "weather")
        .expect("decode");
    assert_eq!(
        decoded.measurement,
        Measurement::Weather {
            temperature_c: Some(-52),
            humidity_pct: Some(71),
            pressure_hpa: Some(1018),
        }
    );
}

#[test]
fn the_server_status_body_keeps_the_subsystem_and_message() {
    let decoded =
        decode_fixture("valid/status_server_body.json", "status").expect("decode");
    assert_eq!(
        decoded.measurement,
        Measurement::Status {
            subsystem: 0,
            result: RESULT_FAILED
        }
    );
    assert_eq!(decoded.subsystem, Some("upstream"));
    assert_eq!(decoded.message, Some("open-meteo timed out"));
}

#[test]
fn the_heartbeat_body_decodes() {
    let decoded =
        decode_fixture("valid/status_heartbeat.json", "status").expect("decode");
    assert_eq!(
        decoded.measurement,
        Measurement::Status {
            subsystem: 0,
            result: RESULT_STARTED
        }
    );
    assert_eq!(decoded.subsystem, Some("lifecycle"));
    assert_eq!(decoded.message, Some("up"));
}

// ----- rejected requests produce the documented error -----------------

#[test]
fn the_server_rejections_decode_as_errors() {
    // (fixture, endpoint type, expected decode error) — bodies taken
    // from the service's own rejection tests where it has one.
    for (path, ty, expected) in [
        // decibel: a string where a number belongs, then a missing field.
        (
            "invalid/decibel_dba_string.json",
            "decibel",
            DecodeError::TypeMismatch("dba"),
        ),
        (
            "invalid/decibel_missing_dba.json",
            "decibel",
            DecodeError::MissingField("dba"),
        ),
        // The server's Float32 column accepts 6553.6; the device model
        // (u16 0.1 dB) refuses — decoders on either side must fail it.
        (
            "invalid/decibel_dba_over_range.json",
            "decibel",
            DecodeError::OutOfRange("dba"),
        ),
        // pax: fractional and negative counts.
        (
            "invalid/pax_occupancy_fractional.json",
            "pax",
            DecodeError::NotWhole("occupancy_count"),
        ),
        (
            "invalid/pax_occupancy_negative.json",
            "pax",
            DecodeError::OutOfRange("occupancy_count"),
        ),
        // weather: a string where a number belongs.
        (
            "invalid/weather_temperature_string.json",
            "weather",
            DecodeError::TypeMismatch("temperature"),
        ),
        // status: the closed result vocabulary, then the empty subsystem.
        (
            "invalid/status_result_unknown.json",
            "status",
            DecodeError::InvalidResult,
        ),
        (
            "invalid/status_subsystem_empty.json",
            "status",
            DecodeError::MissingField("subsystem"),
        ),
        // Structural grammar.
        (
            "invalid/not_an_object.json",
            "decibel",
            DecodeError::NotAnObject,
        ),
        (
            "invalid/trailing_data.json",
            "decibel",
            DecodeError::TrailingData,
        ),
        (
            "invalid/number_leading_zero.json",
            "decibel",
            DecodeError::Malformed,
        ),
    ] {
        let actual = decode_fixture(path, ty);
        assert_eq!(actual, Err(expected), "fixture: {path}");
    }
}

// ----- response fixtures pin the service's wire bytes -----------------

#[test]
fn response_fixtures_match_the_service_writejson_shape() {
    // writeJSON: `{"status":...}` below 300, `{"error":...}` otherwise;
    // json.Encoder.Encode appends the trailing newline. These are the
    // bytes the HTTP transport must parse and classify.
    for (path, golden) in [
        ("responses/accepted.json", "{\"status\":\"accepted\"}\n"),
        (
            "responses/error_subsystem_required.json",
            "{\"error\":\"subsystem is required\"}\n",
        ),
        (
            "responses/error_result_vocabulary.json",
            "{\"error\":\"result must be one of [started stopped success degraded failed]\"}\n",
        ),
        // The invalid-body message embeds Go's json error text for this
        // exact input; the prefix is the stable contract.
        (
            "responses/error_invalid_body.json",
            "{\"error\":\"invalid request body: invalid character 'x' after top-level value\"}\n",
        ),
        (
            "responses/error_sensor_id_uuid.json",
            "{\"error\":\"sensor_id must be a UUID\"}\n",
        ),
        (
            "responses/error_unauthorized.json",
            "{\"error\":\"invalid sensor credentials\"}\n",
        ),
        (
            "responses/error_too_large.json",
            "{\"error\":\"request body is too large\"}\n",
        ),
        (
            "responses/error_storage_unavailable.json",
            "{\"error\":\"measurement storage is unavailable\"}\n",
        ),
        (
            "responses/error_payload_no_credentials.json",
            "{\"error\":\"payload does not carry credentials\"}\n",
        ),
    ] {
        let body = fixture(path);
        assert_eq!(
            core::str::from_utf8(&body).expect("utf8"),
            golden,
            "fixture: {path}"
        );
    }
}
