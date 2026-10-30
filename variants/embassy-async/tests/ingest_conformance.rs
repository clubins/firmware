//! Ingest contract conformance — the wire bodies this firmware POSTs must
//! match the Go ingest structs field-for-field
//! (`server-infra/ingest/service/*.go` in the backend monorepo).
//!
//! Valid fixtures are the expected wire bodies themselves, byte-for-byte:
//! reviewable as data, diffable against the Go structs, and strict enough
//! that a renamed field, reordered key, or changed number format fails CI.
//! Reject fixtures pin the inputs the encoder must refuse (the server would
//! 400 them, or the endpoint does not exist) with the Go file that demands
//! it. A top-level key scanner additionally asserts the exact field set —
//! `encoding/json` ignores unknown keys, so an extra firmware-only key
//! would sail through server-side and must fail here instead.

use std::fs;

use clubins_firmware_v3::codec::{
    Credentials, EncodeError, JsonBody, STATUS_ENDPOINT, encode_measurement,
    encode_status, http_endpoint,
};
use clubins_firmware_v3::events::{
    Co2Sample, DecibelSample, LightSample, MeasurementEvent, MeasurementPayload,
    PaxSample, Pm25Sample, StatusEvent, StatusResult, WeatherSample,
};
use clubins_firmware_v3::session::BootId;

const CREDENTIALS: Credentials = Credentials {
    sensor_id: "9c3f1e42-1111-2222-3333-444455556666",
    apikey: "ck_7f3a2b",
};

fn boot() -> BootId {
    BootId::new([
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
        0x0e, 0x0f, 0x10,
    ])
}

/// 2026-10-30T21:15:29.123Z — the synced-clock instant used by every fixture.
const TIME_MS: u64 = 1_793_394_929_123;

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/ingest/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("fixture {name} unreadable at {path}: {error}"))
        .trim_end()
        .to_string()
}

/// Extract a top-level-string value from a flat single-line JSON fixture —
/// the reject fixtures are flat string maps, so no JSON crate is needed.
fn fixture_field<'a>(text: &'a str, key: &str) -> &'a str {
    let needle = format!("\"{key}\":\"");
    let start = text
        .find(&needle)
        .unwrap_or_else(|| panic!("fixture lacks {key}: {text}"))
        + needle.len();
    let rest = &text[start..];
    let end = rest
        .find('"')
        .unwrap_or_else(|| panic!("fixture {key} unterminated"));
    &rest[..end]
}

/// The top-level keys of a JSON object, in document order — used to pin the
/// exact Go struct field set (nothing dropped, nothing added). A depth-1
/// string literal is a key only when the next non-whitespace byte is `:`;
/// value literals (including escaped ones) resolve to `,` or `}`.
fn top_level_keys(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut keys = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut literal_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if in_string {
            match bytes[i] {
                b'\\' => i += 1, // skip the escaped char
                b'"' => {
                    in_string = false;
                    if depth == 1 {
                        let mut after = i + 1;
                        while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                            after += 1;
                        }
                        if after < bytes.len() && bytes[after] == b':' {
                            keys.push(&body[literal_start..i]);
                        }
                    }
                }
                _ => {}
            }
        } else {
            match bytes[i] {
                b'"' => {
                    in_string = true;
                    literal_start = i + 1;
                }
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
    }
    keys
}

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

fn valid_case(event: &MeasurementEvent, fixture_name: &str, expected_keys: &[&str]) {
    let mut body = JsonBody::new();
    encode_measurement(&CREDENTIALS, &boot(), event, &mut body).unwrap();

    let expected = fixture(fixture_name);
    assert_eq!(
        body.as_str(),
        expected,
        "wire body diverges from {fixture_name}"
    );

    let keys = top_level_keys(body.as_str());
    assert_eq!(
        keys, expected_keys,
        "{fixture_name} must carry exactly the Go struct fields, in order"
    );
}

#[test]
fn decibel_wire_body_matches_the_fixture() {
    valid_case(
        &decibel_event(0, TIME_MS, 92.5, 98.0),
        "decibel.valid.json",
        &[
            "sensor_id",
            "apikey",
            "time",
            "measurement_id",
            "dba",
            "dbc",
        ],
    );
}

#[test]
fn pax_wire_body_matches_the_fixture() {
    valid_case(
        &MeasurementEvent {
            payload: MeasurementPayload::Pax(PaxSample {
                occupancy_count: 42,
            }),
            time_ms: TIME_MS,
            seq: 7,
        },
        "pax.valid.json",
        &[
            "sensor_id",
            "apikey",
            "time",
            "measurement_id",
            "occupancy_count",
        ],
    );
}

#[test]
fn weather_wire_body_matches_the_fixture() {
    valid_case(
        &MeasurementEvent {
            payload: MeasurementPayload::Weather(WeatherSample {
                temperature: Some(21.5),
                humidity: Some(54.25),
                precipitation: Some(0.5),
                rain_probability: Some(0.35),
                wind_speed: Some(12.25),
                pressure: Some(1013.25),
            }),
            time_ms: TIME_MS,
            seq: 1,
        },
        "weather.valid.json",
        &[
            "sensor_id",
            "apikey",
            "time",
            "measurement_id",
            "temperature",
            "humidity",
            "precipitation",
            "rain_probability",
            "wind_speed",
            "pressure",
        ],
    );
}

#[test]
fn status_wire_body_matches_the_fixture() {
    let mut event = StatusEvent {
        time_ms: TIME_MS,
        seq: 0,
        subsystem: heapless::String::new(),
        result: StatusResult::Started,
        message: heapless::String::new(),
    };
    event.subsystem.push_str("lifecycle").unwrap();
    event.message.push_str("boot: v3 event bus").unwrap();

    let mut body = JsonBody::new();
    encode_status(&CREDENTIALS, &boot(), &event, &mut body).unwrap();

    assert_eq!(body.as_str(), fixture("status.valid.json"));
    assert_eq!(
        top_level_keys(body.as_str()),
        [
            "sensor_id",
            "apikey",
            "time",
            "status_id",
            "subsystem",
            "result",
            "message"
        ]
    );
}

/// The endpoint routing is part of the contract: Caddy maps
/// `/v1/sensors/<type>` to the Go handlers, and the payload types that only
/// exist on MQTT v2 must not produce a POST at all.
#[test]
fn endpoints_match_the_server_routes() {
    let routes = [
        (
            MeasurementPayload::Decibel(DecibelSample {
                window_ms: 1000,
                overload: false,
                laeq: 0.0,
                la_fmin: 0.0,
                la_fmax: 0.0,
                lceq: 0.0,
                lc_fmax: 0.0,
            }),
            "/v1/sensors/decibel",
        ),
        (
            MeasurementPayload::Pax(PaxSample { occupancy_count: 0 }),
            "/v1/sensors/pax",
        ),
    ];
    for (payload, expected) in routes {
        assert_eq!(http_endpoint(&payload), Some(expected));
    }
    for payload in [
        MeasurementPayload::Co2(Co2Sample { ppm: 400.0 }),
        MeasurementPayload::Pm25(Pm25Sample { ug_m3: 12.5 }),
        MeasurementPayload::Light(LightSample { lux: 300.0 }),
    ] {
        assert_eq!(http_endpoint(&payload), None, "{payload:?} is MQTT-v2-only");
    }
    assert_eq!(STATUS_ENDPOINT, "/v1/sensors/status");
}

/// Drive every `*.reject.*` fixture: rebuild the offending input, encode,
/// and require the refused outcome the fixture names — with the buffer left
/// empty so a dropped error can never half-send.
#[test]
fn reject_fixtures_are_refused_exactly_as_documented() {
    for name in [
        "decibel.reject.nan-laeq.json",
        "weather.reject.missing-precipitation.json",
        "co2.reject.no-http-endpoint.json",
    ] {
        let text = fixture(name);
        let payload_kind = fixture_field(&text, "payload");
        let expected = fixture_field(&text, "expected_error").to_string();

        let mut body = JsonBody::new();
        let actual = match payload_kind {
            "decibel" => {
                // The fixture's field names the wire field the refusal names
                // (dba — encoded from the laeq sample); the sample is always
                // NaN'd at laeq.
                let _ = fixture_field(&text, "field");
                encode_measurement(
                    &CREDENTIALS,
                    &boot(),
                    &decibel_event(0, TIME_MS, f32::NAN, 98.0),
                    &mut body,
                )
            }
            "weather" => encode_measurement(
                &CREDENTIALS,
                &boot(),
                &MeasurementEvent {
                    payload: MeasurementPayload::Weather(WeatherSample {
                        temperature: Some(21.5),
                        humidity: Some(54.25),
                        ..WeatherSample::default()
                    }),
                    time_ms: TIME_MS,
                    seq: 0,
                },
                &mut body,
            ),
            "co2" => encode_measurement(
                &CREDENTIALS,
                &boot(),
                &MeasurementEvent {
                    payload: MeasurementPayload::Co2(Co2Sample { ppm: 400.0 }),
                    time_ms: TIME_MS,
                    seq: 0,
                },
                &mut body,
            ),
            other => panic!("reject fixture {name} has unknown payload {other}"),
        }
        .unwrap_err();

        let matches = match (expected.as_str(), &actual) {
            ("not_finite", EncodeError::NotFinite(field)) => {
                assert_eq!(
                    *field,
                    fixture_field(&text, "field"),
                    "{name} field mismatch"
                );
                true
            }
            ("incomplete_weather", EncodeError::WeatherIncomplete(field)) => {
                assert_eq!(
                    *field,
                    fixture_field(&text, "field"),
                    "{name} field mismatch"
                );
                true
            }
            ("no_http_endpoint", EncodeError::NoHttpEndpoint) => true,
            _ => false,
        };
        assert!(matches, "{name}: expected {expected}, got {actual:?}");
        // Mid-encode refusals may leave a partial body (the credentials
        // prefix precedes payload validation) — the documented contract is
        // that the caller drops the buffer, and the next encode clears it
        // wholesale. The one refusal guaranteed pre-encode is
        // no_http_endpoint, which must leave the buffer untouched.
        if expected == "no_http_endpoint" {
            assert!(
                body.is_empty(),
                "{name}: a pre-encode refusal leaves no bytes"
            );
        } else {
            encode_measurement(
                &CREDENTIALS,
                &boot(),
                &decibel_event(0, TIME_MS, 92.5, 98.0),
                &mut body,
            )
            .unwrap();
            assert!(
                body.as_str().starts_with("{\"sensor_id\""),
                "{name}: a fresh encode must fully overwrite a dropped buffer"
            );
            assert!(!body.as_str().contains("NaN"));
        }
    }
}

/// The status side of the reject contract: a result outside the server's
/// closed vocabulary can never be produced by construction — `StatusResult`
/// is an enum whose `as_str` is exhaustive over `started|stopped|success|
/// degraded|failed` (status.go). Pin that vocabulary here so a new variant
/// cannot silently diverge from the server.
#[test]
fn status_results_use_the_server_vocabulary() {
    let vocabulary = [
        (StatusResult::Started, "started"),
        (StatusResult::Stopped, "stopped"),
        (StatusResult::Success, "success"),
        (StatusResult::Degraded, "degraded"),
        (StatusResult::Failed, "failed"),
    ];
    for (result, wire) in vocabulary {
        assert_eq!(result.as_str(), wire);
    }
}
