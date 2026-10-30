//! The HTTP JSON ingest codec (`transport-http` feature).
//!
//! Encodes an [`IngestEvent`] into the exact body the ingest service
//! accepts and decodes a body back into one. Field names and validation
//! mirror `server-infra/ingest/service/*.go` field for field:
//!
//! | Endpoint | Body fields (in Go struct order) |
//! |---|---|
//! | `POST /v1/sensors/decibel` | `sensor_id`, `apikey`, `time`, `measurement_id`, `dba`, `dbc` |
//! | `POST /v1/sensors/pax` | `sensor_id`, `apikey`, `time`, `measurement_id`, `occupancy_count` |
//! | `POST /v1/sensors/weather` | `sensor_id`, `apikey`, `time`, `measurement_id`, `temperature`, `humidity`, `precipitation`, `rain_probability`, `wind_speed`, `pressure` |
//! | `POST /v1/sensors/status` | `sensor_id`, `apikey`, `time`, `status_id`, `subsystem`, `result`, `message` |
//!
//! Decoder semantics mirror `encoding/json` plus the handlers, so a body
//! this decoder accepts is a body the server accepts (and vice versa):
//! unknown keys are ignored (forward-compatible telemetry — the legacy v1
//! firmware's dropped `dba_min`/`dba_max` fields decode cleanly), a known
//! key with the wrong JSON type is an error, duplicate keys resolve to the
//! last occurrence, and value rules match the Go validators (`occupancy_
//! _count` finite, whole, 0…2³²−1; floats finite and float32-safe; `result`
//! in the closed vocabulary; `subsystem` non-empty). Like the server, the
//! decoder does *not* validate `time` or the row ids — absent or malformed
//! ones fall back server-side (`rowTime`/`rowID` generate UUIDv7 and now).
//!
//! JSON is parsed by a small strict parser — the crate's only dependency is
//! the TOML parser, and serde would double the size of what this needs.
//! Depth is capped so a hostile or corrupt body cannot overflow the stack
//! on-target.
//!
//! `no_std`-compatible with `alloc`; host-tested against the fixtures in
//! `tests/fixtures/ingest/`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write as _;

use crate::measurement::{InvalidMeasurement, Measurement, StatusResult};

/// Public ingest paths (Caddy strips the `/v1` prefix before proxying —
/// `modules/api/ingest.nix`; the Go mux registers the stripped forms).
pub const DECIBEL_PATH: &str = "/v1/sensors/decibel";
pub const PAX_PATH: &str = "/v1/sensors/pax";
pub const WEATHER_PATH: &str = "/v1/sensors/weather";
pub const STATUS_PATH: &str = "/v1/sensors/status";

/// Maximum JSON nesting the decoder will walk. Device bodies are flat
/// objects; the cap exists so corrupt input cannot exhaust the stack.
const MAX_DEPTH: usize = 32;

/// The credentials block every body carries flat (`service.go`
/// `credentials`). Identity comes from device configuration (NVS), never
/// from the pipeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Credentials {
    /// The sensor's UUID, in the canonical 8-4-4-4-12 form the server
    /// parses (`uuid.Parse`; the decoder accepts its alternate spellings).
    pub sensor_id: String,
    /// The per-sensor API key. Any string is decodable — the server
    /// authenticates afterwards and collapses every failure to 401.
    pub apikey: String,
    /// RFC 3339 UTC timestamp of the reading. Optional: the server falls
    /// back to now for anything absent or unparseable (`rowTime`).
    pub time: Option<String>,
}

/// One ingest request: credentials, the optional row id (`measurement_id`,
/// or `status_id` for status reports — the server generates a UUIDv7 when
/// absent or malformed), and the measurement itself.
#[derive(Clone, Debug, PartialEq)]
pub struct IngestEvent {
    pub credentials: Credentials,
    pub row_id: Option<String>,
    pub measurement: Measurement,
}

/// An encoded request: POST `body` to `path`.
#[derive(Clone, Debug, PartialEq)]
pub struct Encoded {
    pub path: &'static str,
    pub body: String,
}

/// Why a request could not be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The measurement broke a model invariant (non-finite float, empty
    /// subsystem) — unreachable through the constructors, caught for
    /// hand-built struct literals.
    Invalid(InvalidMeasurement),
    /// `credentials.sensor_id` is not a UUID — the server 400s the request
    /// before even authenticating.
    SensorIdNotUuid,
    /// The measurement type has no HTTP ingest contract. The new types
    /// (`co2`, `pm25`, `light`) and environmental readings travel on MQTT
    /// per the one-topic-per-type rule until the backend grows receivers;
    /// encoding them as HTTP bodies the server will 400/404 would be
    /// conformance theater.
    NoHttpContract { type_name: &'static str },
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Invalid(e) => write!(f, "invalid measurement: {e}"),
            Self::SensorIdNotUuid => write!(f, "sensor_id must be a UUID"),
            Self::NoHttpContract { type_name } => write!(
                f,
                "the '{type_name}' type has no HTTP ingest endpoint; \
                 it travels on MQTT (one topic per type)"
            ),
        }
    }
}

/// Why a body could not be decoded. Mirrors the server's failure classes:
/// every variant here corresponds to a response the ingest service can
/// produce for the same body.
#[derive(Clone, Debug, PartialEq)]
pub enum DecodeError {
    /// The text is not valid JSON (the server: 400 "invalid request body").
    Syntax(String),
    /// A known key arrived with the wrong JSON type (the server: 400 via
    /// `encoding/json`'s unmarshal type error).
    WrongType {
        field: &'static str,
        want: &'static str,
        got: &'static str,
    },
    /// A required field is absent (the server: 400 "<field> is required").
    MissingField { field: &'static str },
    /// A field arrived with the right shape but an unusable value (the
    /// server: 400 from the handler's own validators).
    BadValue {
        field: &'static str,
        why: &'static str,
    },
    /// The path does not name an ingest endpoint.
    UnknownPath(String),
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Syntax(why) => write!(f, "invalid JSON: {why}"),
            Self::WrongType { field, want, got } => {
                write!(f, "'{field}' must be {want}, got {got}")
            }
            Self::MissingField { field } => write!(f, "'{field}' is required"),
            Self::BadValue { field, why } => write!(f, "'{field}' {why}"),
            Self::UnknownPath(path) => write!(f, "unknown endpoint '{path}'"),
        }
    }
}

/// The ingest service's response bodies (`writeJSON`): exactly one key,
/// `status` on success (`202 {"status":"accepted"}`) or `error` otherwise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngestResponse {
    Status(String),
    Error(String),
}

/// A parsed JSON value — only what mapping needs; unknown subtrees are
/// skipped without being retained.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// Human-readable JSON kind for type errors.
fn kind_of(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "a bool",
        Json::Num(_) => "a number",
        Json::Str(_) => "a string",
        Json::Arr(_) => "an array",
        Json::Obj(_) => "an object",
    }
}

/// Strict JSON number grammar: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
fn scan_number(src: &str, pos: &mut usize) -> Result<f64, DecodeError> {
    let bytes = src.as_bytes();
    let start = *pos;
    if bytes.get(*pos) == Some(&b'-') {
        *pos += 1;
    }
    match bytes.get(*pos) {
        Some(b'0') => *pos += 1,
        Some(c) if c.is_ascii_digit() => {
            while bytes.get(*pos).is_some_and(|c| c.is_ascii_digit()) {
                *pos += 1;
            }
        }
        _ => {
            *pos = start;
            return Err(Syntax::at(src, *pos, "invalid number"));
        }
    }
    if bytes.get(*pos) == Some(&b'.') {
        *pos += 1;
        if !bytes.get(*pos).is_some_and(|c| c.is_ascii_digit()) {
            *pos = start;
            return Err(Syntax::at(src, *pos, "invalid number"));
        }
        while bytes.get(*pos).is_some_and(|c| c.is_ascii_digit()) {
            *pos += 1;
        }
    }
    if matches!(bytes.get(*pos), Some(b'e') | Some(b'E')) {
        *pos += 1;
        if matches!(bytes.get(*pos), Some(b'+') | Some(b'-')) {
            *pos += 1;
        }
        if !bytes.get(*pos).is_some_and(|c| c.is_ascii_digit()) {
            *pos = start;
            return Err(Syntax::at(src, *pos, "invalid number"));
        }
        while bytes.get(*pos).is_some_and(|c| c.is_ascii_digit()) {
            *pos += 1;
        }
    }
    // The grammar above guarantees `parse` succeeds; overflow to inf is a
    // value problem for the field mapper, not a syntax problem.
    src[start..*pos]
        .parse::<f64>()
        .map_err(|_| Syntax::at(src, start, "invalid number"))
}

/// Positioned syntax errors, so the scanners can point at the failure site
/// without threading `src` through every helper signature.
struct Syntax;

impl Syntax {
    fn at(_src: &str, pos: usize, why: &str) -> DecodeError {
        DecodeError::Syntax(alloc::format!("{why} at byte {pos}"))
    }
}

/// Four hex digits of a `\u` escape; all four must be hex (JSON forbids
/// `+123`, and `from_str_radix` would otherwise accept it). Byte-level: a
/// multi-byte char inside the window must fail the hex check, not panic a
/// str slice.
fn hex4(src: &str, pos: &mut usize) -> Result<u16, DecodeError> {
    let bytes = src.as_bytes();
    match bytes.get(*pos..*pos + 4) {
        Some(window) if window.iter().all(|b| b.is_ascii_hexdigit()) => {
            let digits =
                core::str::from_utf8(window).expect("ASCII hex digits are UTF-8");
            let value = u16::from_str_radix(digits, 16).unwrap_or(0);
            *pos += 4;
            Ok(value)
        }
        _ => Err(Syntax::at(src, *pos, "invalid \\u escape")),
    }
}

/// One `\uXXXX` escape, with surrogate-pair combining. Lone surrogates
/// decode to U+FFFD, matching `encoding/json`'s tolerance.
fn unicode_escape(src: &str, pos: &mut usize) -> Result<char, DecodeError> {
    let first = hex4(src, pos)?;
    let combined = match first {
        0xD800..=0xDBFF => {
            if src[*pos..].starts_with("\\u") {
                let save = *pos;
                *pos += 2;
                match hex4(src, pos)? {
                    second @ 0xDC00..=0xDFFF => {
                        let bits =
                            ((first as u32 - 0xD800) << 10) | (second as u32 - 0xDC00);
                        char::from_u32(0x10000 + bits).unwrap_or('\u{FFFD}')
                    }
                    _ => {
                        // Not a low surrogate: rewind and emit the
                        // replacement char, then reparse the digits as a
                        // fresh escape on the next call.
                        *pos = save;
                        '\u{FFFD}'
                    }
                }
            } else {
                '\u{FFFD}'
            }
        }
        0xDC00..=0xDFFF => '\u{FFFD}',
        _ => char::from_u32(first as u32).unwrap_or('\u{FFFD}'),
    };
    Ok(combined)
}

/// Append `value` as a JSON string literal (quoted, escaped). Non-ASCII
/// passes through as UTF-8, which JSON allows.
fn push_escaped(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse one JSON value (any type) at `*pos`, which must sit on the value's
/// first byte. `depth` guards recursion.
fn parse_value(src: &str, pos: &mut usize, depth: usize) -> Result<Json, DecodeError> {
    if depth > MAX_DEPTH {
        return Err(Syntax::at(src, *pos, "nesting deeper than 32 levels"));
    }
    let bytes = src.as_bytes();
    match bytes.get(*pos) {
        Some(b'{') => {
            *pos += 1;
            let mut fields = Vec::new();
            skip_ws(src, pos);
            if bytes.get(*pos) == Some(&b'}') {
                *pos += 1;
                return Ok(Json::Obj(fields));
            }
            loop {
                skip_ws(src, pos);
                let key = match parse_value(src, pos, depth)? {
                    Json::Str(key) => key,
                    other => {
                        return Err(Syntax::at(
                            src,
                            *pos,
                            &alloc::format!(
                                "expected object key, got {}",
                                kind_of(&other)
                            ),
                        ));
                    }
                };
                skip_ws(src, pos);
                if bytes.get(*pos) != Some(&b':') {
                    return Err(Syntax::at(src, *pos, "expected ':'"));
                }
                *pos += 1;
                skip_ws(src, pos);
                let value = parse_value(src, pos, depth + 1)?;
                fields.push((key, value));
                skip_ws(src, pos);
                match bytes.get(*pos) {
                    Some(b',') => *pos += 1,
                    Some(b'}') => {
                        *pos += 1;
                        return Ok(Json::Obj(fields));
                    }
                    _ => return Err(Syntax::at(src, *pos, "expected ',' or '}'")),
                }
            }
        }
        Some(b'[') => {
            *pos += 1;
            let mut items = Vec::new();
            skip_ws(src, pos);
            if bytes.get(*pos) == Some(&b']') {
                *pos += 1;
                return Ok(Json::Arr(items));
            }
            loop {
                skip_ws(src, pos);
                items.push(parse_value(src, pos, depth + 1)?);
                skip_ws(src, pos);
                match bytes.get(*pos) {
                    Some(b',') => *pos += 1,
                    Some(b']') => {
                        *pos += 1;
                        return Ok(Json::Arr(items));
                    }
                    _ => return Err(Syntax::at(src, *pos, "expected ',' or ']'")),
                }
            }
        }
        Some(b'"') => {
            *pos += 1;
            let mut out = String::new();
            loop {
                let Some(c) = src[*pos..].chars().next() else {
                    return Err(Syntax::at(src, *pos, "unterminated string"));
                };
                match c {
                    '"' => {
                        *pos += 1;
                        return Ok(Json::Str(out));
                    }
                    '\\' => {
                        *pos += 1;
                        match bytes.get(*pos) {
                            Some(b'"') => {
                                out.push('"');
                                *pos += 1;
                            }
                            Some(b'\\') => {
                                out.push('\\');
                                *pos += 1;
                            }
                            Some(b'/') => {
                                out.push('/');
                                *pos += 1;
                            }
                            Some(b'b') => {
                                out.push('\u{0008}');
                                *pos += 1;
                            }
                            Some(b'f') => {
                                out.push('\u{000C}');
                                *pos += 1;
                            }
                            Some(b'n') => {
                                out.push('\n');
                                *pos += 1;
                            }
                            Some(b'r') => {
                                out.push('\r');
                                *pos += 1;
                            }
                            Some(b't') => {
                                out.push('\t');
                                *pos += 1;
                            }
                            Some(b'u') => {
                                *pos += 1;
                                out.push(unicode_escape(src, pos)?);
                            }
                            _ => return Err(Syntax::at(src, *pos, "invalid escape")),
                        }
                    }
                    c if (c as u32) < 0x20 => {
                        return Err(Syntax::at(
                            src,
                            *pos,
                            "raw control character in string",
                        ));
                    }
                    c => {
                        out.push(c);
                        *pos += c.len_utf8();
                    }
                }
            }
        }
        Some(b't') => literal(src, pos, "true").map(|_| Json::Bool(true)),
        Some(b'f') => literal(src, pos, "false").map(|_| Json::Bool(false)),
        Some(b'n') => literal(src, pos, "null").map(|_| Json::Null),
        Some(c) if c.is_ascii_digit() || *c == b'-' => {
            scan_number(src, pos).map(Json::Num)
        }
        _ => Err(Syntax::at(src, *pos, "expected a JSON value")),
    }
}

/// Consume a known literal (`true`/`false`/`null`).
fn literal(src: &str, pos: &mut usize, word: &str) -> Result<(), DecodeError> {
    if src[*pos..].starts_with(word) {
        *pos += word.len();
        Ok(())
    } else {
        Err(Syntax::at(src, *pos, "expected a JSON value"))
    }
}

fn skip_ws(src: &str, pos: &mut usize) {
    while matches!(src.as_bytes().get(*pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        *pos += 1;
    }
}

/// Parse a complete document: one value, then nothing but whitespace
/// (`encoding/json` rejects trailing data).
fn parse_document(src: &str) -> Result<Json, DecodeError> {
    let mut pos = 0;
    skip_ws(src, &mut pos);
    let value = parse_value(src, &mut pos, 0)?;
    skip_ws(src, &mut pos);
    if pos != src.len() {
        return Err(Syntax::at(src, pos, "trailing data after the JSON value"));
    }
    Ok(value)
}

/// Last occurrence of `key` — `encoding/json` resolves duplicates to the
/// last one.
fn last_field<'j>(obj: &'j [(String, Json)], key: &str) -> Option<&'j Json> {
    obj.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn opt_str(
    obj: &[(String, Json)],
    field: &'static str,
) -> Result<Option<String>, DecodeError> {
    match last_field(obj, field) {
        None => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s.clone())),
        Some(other) => Err(DecodeError::WrongType {
            field,
            want: "a string",
            got: kind_of(other),
        }),
    }
}

fn opt_num(
    obj: &[(String, Json)],
    field: &'static str,
) -> Result<Option<f64>, DecodeError> {
    match last_field(obj, field) {
        None => Ok(None),
        Some(Json::Num(v)) => Ok(Some(*v)),
        Some(other) => Err(DecodeError::WrongType {
            field,
            want: "a number",
            got: kind_of(other),
        }),
    }
}

fn req_num(obj: &[(String, Json)], field: &'static str) -> Result<f64, DecodeError> {
    opt_num(obj, field)?.ok_or(DecodeError::MissingField { field })
}

/// Narrow to f32 under the service's `measurementValue` rule: finite and
/// within float32 range. (Every f64 ≤ f32::MAX narrows to a finite f32 —
/// f64 represents all f32 exactly.)
fn f32_safe(value: f64) -> Option<f32> {
    if value.is_finite() && value.abs() <= f32::MAX as f64 {
        Some(value as f32)
    } else {
        None
    }
}

/// A required float32 measurement field, with the server's two distinct
/// failure messages kept apart: out-of-JSON-range numbers fail at decode
/// (`1e400`), merely-out-of-float32 ones at validation (`1e300`).
fn req_f32(obj: &[(String, Json)], field: &'static str) -> Result<f32, DecodeError> {
    let value = req_num(obj, field)?;
    if !value.is_finite() {
        return Err(DecodeError::BadValue {
            field,
            why: "number is out of JSON range",
        });
    }
    f32_safe(value).ok_or(DecodeError::BadValue {
        field,
        why: "must be a finite 32-bit float",
    })
}

/// The `occupancyCount` validator: finite, 0…2³²−1, whole.
fn req_u32(obj: &[(String, Json)], field: &'static str) -> Result<u32, DecodeError> {
    let value = req_num(obj, field)?;
    if !value.is_finite() {
        return Err(DecodeError::BadValue {
            field,
            why: "must be a finite number",
        });
    }
    if !(0.0..=(u32::MAX as f64)).contains(&value) {
        return Err(DecodeError::BadValue {
            field,
            why: "must be between 0 and 4294967295",
        });
    }
    // `f64::fract` is std-only; the core `Rem` impl does the same job.
    if value % 1.0 != 0.0 {
        return Err(DecodeError::BadValue {
            field,
            why: "must be a whole number",
        });
    }
    Ok(value as u32)
}

/// Canonical-form UUID check (`8-4-4-4-12`, case-insensitive hex), plus the
/// alternate spellings `google/uuid`'s `Parse` accepts: a 32-hex naked form,
/// a `urn:uuid:` prefix, and brace wrapping. The firmware always sends the
/// canonical form; the alternatives exist so the decoder accepts bodies the
/// server accepts.
fn is_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    let canonical = |b: &[u8]| {
        b.len() == 36
            && b.iter().enumerate().all(|(i, c)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    *c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
    };
    match bytes.len() {
        36 => canonical(bytes),
        32 => bytes.iter().all(|c| c.is_ascii_hexdigit()),
        // The guard compares ASCII bytes, so the 9-byte str slices below
        // are on char boundaries.
        45 => bytes[..9].eq_ignore_ascii_case(b"urn:uuid:") && canonical(&bytes[9..]),
        38 => bytes[0] == b'{' && bytes[37] == b'}' && canonical(&bytes[1..37]),
        _ => false,
    }
}

/// Encode a request for the ingest service. The body's field order is the
/// Go structs' order (credentials promoted first), so encoded output is
/// byte-stable and goldens are meaningful.
pub fn encode(event: &IngestEvent) -> Result<Encoded, EncodeError> {
    event.measurement.check().map_err(EncodeError::Invalid)?;
    if !is_uuid(&event.credentials.sensor_id) {
        return Err(EncodeError::SensorIdNotUuid);
    }

    // Path and the row-id key name come from the measurement type. Types
    // without an HTTP contract are rejected here — honestly, with a reason.
    let (path, row_key) = match &event.measurement {
        Measurement::Decibel { .. } => (DECIBEL_PATH, "measurement_id"),
        Measurement::Pax { .. } => (PAX_PATH, "measurement_id"),
        Measurement::Weather { .. } => (WEATHER_PATH, "measurement_id"),
        Measurement::Status(_) => (STATUS_PATH, "status_id"),
        other => {
            return Err(EncodeError::NoHttpContract {
                type_name: other.type_name(),
            });
        }
    };

    let mut body = Body::new();
    body.string("sensor_id", &event.credentials.sensor_id);
    body.string("apikey", &event.credentials.apikey);
    if let Some(time) = &event.credentials.time {
        body.string("time", time);
    }
    if let Some(row_id) = &event.row_id {
        body.string(row_key, row_id);
    }
    match &event.measurement {
        Measurement::Decibel { dba, dbc } => {
            body.f32("dba", *dba);
            body.f32("dbc", *dbc);
        }
        Measurement::Pax { occupancy_count } => {
            body.u32("occupancy_count", *occupancy_count);
        }
        Measurement::Weather {
            temperature,
            humidity,
            precipitation,
            rain_probability,
            wind_speed,
            pressure,
        } => {
            body.f32("temperature", *temperature);
            body.f32("humidity", *humidity);
            body.f32("precipitation", *precipitation);
            body.f32("rain_probability", *rain_probability);
            body.f32("wind_speed", *wind_speed);
            body.f32("pressure", *pressure);
        }
        Measurement::Status(report) => {
            body.string("subsystem", &report.subsystem);
            body.string("result", report.result.name());
            if let Some(message) = &report.message {
                body.string("message", message);
            }
        }
        // Unreachable: the first match returned NoHttpContract for these.
        _ => unreachable!("unwired measurement types never reach field encoding"),
    }

    Ok(Encoded {
        path,
        body: body.finish(),
    })
}

/// Append a float32 field value as a JSON number. Mirrors `encoding/json`'s
/// format rule (switch to exponent form outside 1e-6…1e21) with one
/// deliberate divergence: exponent-form values are spelled at f64
/// precision. Rust's shortest *f32* digits for f32::MAX ("34028235…")
/// f64-parse to a value just above `f32::MAX` — exactly what the reference
/// server's `measurementValue` rejects (`math.Abs(*value) > math.MaxFloat32`
/// on the decoded f64, service.go:207) — while the f64-exact widening
/// ("3.4028234663852886e38") parses back onto the same f64 and narrows to
/// the same f32, so every f32 round-trips through the server's domain.
fn push_f32_json(out: &mut String, value: f32) {
    let abs = value.abs();
    // Exponent form iff non-zero and outside 1e-6…1e21, written as an
    // explicit zero guard plus negated range: clippy's bare
    // `!(1e-6..1e21).contains(&abs)` would also flip zero into exponent
    // form (0.0 < 1e-6), changing the encoding from "0" to "0e0".
    let use_exponent = abs != 0.0 && !(1e-6..1e21).contains(&abs);
    if use_exponent {
        let _ = write!(out, "{:e}", value as f64);
    } else {
        let _ = write!(out, "{value}");
    }
}

/// Incremental JSON object builder keeping the Go struct field order.
struct Body {
    out: String,
    first: bool,
}

impl Body {
    fn new() -> Self {
        Self {
            out: String::from("{"),
            first: true,
        }
    }

    fn sep(&mut self) {
        if !self.first {
            self.out.push(',');
        }
        self.first = false;
    }

    fn string(&mut self, key: &str, value: &str) {
        self.sep();
        let _ = write!(self.out, "\"{key}\":");
        push_escaped(&mut self.out, value);
    }

    fn f32(&mut self, key: &str, value: f32) {
        self.sep();
        let _ = write!(self.out, "\"{key}\":");
        push_f32_json(&mut self.out, value);
    }

    fn u32(&mut self, key: &str, value: u32) {
        self.sep();
        let _ = write!(self.out, "\"{key}\":{value}");
    }

    fn finish(mut self) -> String {
        self.out.push('}');
        self.out
    }
}

/// Decode a body for one ingest endpoint. `path` is the public path
/// (`/v1/sensors/decibel`) or the stripped service path the Go mux
/// registers; anything else is [`DecodeError::UnknownPath`].
pub fn decode(path: &str, body: &str) -> Result<IngestEvent, DecodeError> {
    let service_path = path.strip_prefix("/v1").unwrap_or(path);
    let Json::Obj(obj) = parse_document(body)? else {
        return Err(DecodeError::WrongType {
            field: "body",
            want: "a JSON object",
            got: "an array or scalar",
        });
    };

    // Credentials first — every endpoint carries them. `sensor_id` is the
    // only one the server hard-rejects at decode (uuid.Parse); an absent
    // apikey decodes to "" and fails authentication later (401), and time
    // falls back to now.
    let sensor_id = opt_str(&obj, "sensor_id")?
        .ok_or(DecodeError::MissingField { field: "sensor_id" })?;
    if !is_uuid(&sensor_id) {
        return Err(DecodeError::BadValue {
            field: "sensor_id",
            why: "must be a UUID",
        });
    }
    let apikey = opt_str(&obj, "apikey")?.unwrap_or_default();
    let time = opt_str(&obj, "time")?;
    let credentials = Credentials {
        sensor_id,
        apikey,
        time,
    };

    // Row id key differs per endpoint: `measurement_id` everywhere, except
    // status which carries `status_id` (both fall back to a generated
    // UUIDv7 server-side, so both decode to an Option).
    let (row_id, measurement) = match service_path {
        "/sensors/decibel" => {
            let row_id = opt_str(&obj, "measurement_id")?;
            let dba = req_f32(&obj, "dba")?;
            let dbc = req_f32(&obj, "dbc")?;
            // Unreachable error: req_f32 already guarantees finiteness.
            let measurement =
                Measurement::decibel(dba, dbc).map_err(|e| DecodeError::BadValue {
                    field: "dba",
                    why: why_of(e),
                })?;
            (row_id, measurement)
        }
        "/sensors/pax" => {
            let row_id = opt_str(&obj, "measurement_id")?;
            let measurement = Measurement::pax(req_u32(&obj, "occupancy_count")?);
            (row_id, measurement)
        }
        "/sensors/weather" => {
            let row_id = opt_str(&obj, "measurement_id")?;
            // The service validates in this fixed order, so a body missing
            // several fields always reports the same one first.
            let temperature = req_f32(&obj, "temperature")?;
            let humidity = req_f32(&obj, "humidity")?;
            let precipitation = req_f32(&obj, "precipitation")?;
            let rain_probability = req_f32(&obj, "rain_probability")?;
            let wind_speed = req_f32(&obj, "wind_speed")?;
            let pressure = req_f32(&obj, "pressure")?;
            let measurement = Measurement::weather(
                temperature,
                humidity,
                precipitation,
                rain_probability,
                wind_speed,
                pressure,
            )
            .map_err(|e| DecodeError::BadValue {
                field: "temperature",
                why: why_of(e),
            })?;
            (row_id, measurement)
        }
        "/sensors/status" => {
            let row_id = opt_str(&obj, "status_id")?;
            // The server treats absent and empty subsystem identically
            // ("subsystem is required"), so the decoder does too.
            let subsystem = opt_str(&obj, "subsystem")?.unwrap_or_default();
            if subsystem.is_empty() {
                return Err(DecodeError::BadValue {
                    field: "subsystem",
                    why: "is required",
                });
            }
            let result_name = req_str(&obj, "result")?;
            let result =
                StatusResult::lookup(&result_name).ok_or(DecodeError::BadValue {
                    field: "result",
                    why: "must be one of [started stopped success degraded failed]",
                })?;
            let measurement = Measurement::status(crate::measurement::StatusReport {
                status_id: row_id.clone(),
                subsystem,
                result,
                message: opt_str(&obj, "message")?,
            });
            (row_id, measurement)
        }
        other => return Err(DecodeError::UnknownPath(other.to_string())),
    };

    Ok(IngestEvent {
        credentials,
        row_id,
        measurement,
    })
}

/// The (unreachable-through-`req_f32`) constructor failure, as the field
/// message the server would print.
fn why_of(e: InvalidMeasurement) -> &'static str {
    match e {
        InvalidMeasurement::NotFinite { .. } => "must be a finite 32-bit float",
        InvalidMeasurement::SubsystemRequired => "is required",
    }
}

fn req_str(obj: &[(String, Json)], field: &'static str) -> Result<String, DecodeError> {
    opt_str(obj, field)?.ok_or(DecodeError::MissingField { field })
}

/// Decode an ingest service response body (`{"status":"accepted"}` or
/// `{"error":"..."}`). The HTTP status code travels with the response and
/// decides success; this only reads the message.
pub fn decode_response(body: &str) -> Result<IngestResponse, DecodeError> {
    let Json::Obj(obj) = parse_document(body)? else {
        return Err(DecodeError::WrongType {
            field: "body",
            want: "a JSON object",
            got: "an array or scalar",
        });
    };
    if let Some(Json::Str(message)) = last_field(&obj, "error") {
        return Ok(IngestResponse::Error(message.clone()));
    }
    if let Some(Json::Str(message)) = last_field(&obj, "status") {
        return Ok(IngestResponse::Status(message.clone()));
    }
    Err(DecodeError::BadValue {
        field: "body",
        why: "expected a {\"status\"} or {\"error\"} object",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::StatusReport;

    fn decibel_event() -> IngestEvent {
        IngestEvent {
            credentials: Credentials {
                sensor_id: "0a20720e-bf44-4f4c-9a59-d40dd827e7f7".into(),
                apikey: "development-api-key".into(),
                time: Some("2026-08-21T12:00:00Z".into()),
            },
            row_id: Some("019873f4-1234-7abc-8def-0123456789ab".into()),
            measurement: Measurement::decibel(72.4, 78.1).expect("finite"),
        }
    }

    #[test]
    fn encode_is_field_exact_and_byte_stable() {
        let encoded = encode(&decibel_event()).expect("encodes");
        assert_eq!(encoded.path, "/v1/sensors/decibel");
        assert_eq!(
            encoded.body,
            "{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\
             \"apikey\":\"development-api-key\",\
             \"time\":\"2026-08-21T12:00:00Z\",\
             \"measurement_id\":\"019873f4-1234-7abc-8def-0123456789ab\",\
             \"dba\":72.4,\"dbc\":78.1}"
        );
    }

    #[test]
    fn round_trip_preserves_every_event() {
        let event = decibel_event();
        let encoded = encode(&event).expect("encodes");
        let decoded = decode(encoded.path, &encoded.body).expect("decodes");
        assert_eq!(decoded, event);
    }

    #[test]
    fn integer_valued_floats_encode_as_json_integers() {
        // 71.0 must encode as "71" (valid JSON number, what the Go
        // weather test bodies use), not "71.0" — byte-exact goldens depend
        // on the formatter's spelling.
        let weather =
            Measurement::weather(18.4, 71.0, 0.2, 40.0, 12.7, 1018.4).expect("finite");
        let event = IngestEvent {
            credentials: Credentials {
                sensor_id: "630d11c4-57d8-4a72-9899-3eb995f6e147".into(),
                apikey: "development-api-key".into(),
                time: None,
            },
            row_id: None,
            measurement: weather,
        };
        let encoded = encode(&event).expect("encodes");
        assert!(encoded.body.contains("\"humidity\":71"));
        assert!(encoded.body.contains("\"rain_probability\":40"));
        // And it round-trips.
        assert_eq!(decode(encoded.path, &encoded.body).expect("decodes"), event);
    }

    #[test]
    fn extreme_f32_values_round_trip() {
        let event = IngestEvent {
            credentials: decibel_event().credentials,
            row_id: None,
            measurement: Measurement::decibel(f32::MAX, f32::MIN).expect("finite"),
        };
        let encoded = encode(&event).expect("encodes");
        // Exponent-form f64-precision spelling — f32::MAX's shortest-f32
        // decimal would f64-parse above f32::MAX and the server would 400
        // it (see push_f32_json).
        assert!(encoded.body.contains("3.4028234663852886e38"));
        assert_eq!(decode(encoded.path, &encoded.body).expect("decodes"), event);
    }

    #[test]
    fn tiny_and_zero_floats_encode_readable() {
        // 0 keeps 'f' form; subnormals go exponent-form.
        let event = IngestEvent {
            credentials: decibel_event().credentials,
            row_id: None,
            measurement: Measurement::decibel(0.0, 1e-45).expect("finite"),
        };
        let encoded = encode(&event).expect("encodes");
        assert!(encoded.body.contains("\"dba\":0"));
        // 1e-45f32 is the subnormal 2^-149; its f64-widened spelling carries
        // the exact widening digits, not the source literal.
        assert!(encoded.body.contains("e-45"));
        assert_eq!(decode(encoded.path, &encoded.body).expect("decodes"), event);
    }

    #[test]
    fn escapes_round_trip() {
        let mut report =
            StatusReport::new("power", StatusResult::Degraded).expect("non-empty");
        report.message = Some(
            "quote \" backslash \\ newline \n tab \t ctrl \u{0001} umlaut ä emoji 😀"
                .into(),
        );
        let event = IngestEvent {
            credentials: decibel_event().credentials,
            row_id: None,
            measurement: Measurement::status(report),
        };
        let encoded = encode(&event).expect("encodes");
        assert_eq!(decode(encoded.path, &encoded.body).expect("decodes"), event);
    }

    #[test]
    fn surrogate_pairs_decode_to_the_codepoint() {
        let mut pos = 0;
        let src = "\"\\ud834\\udd1e\""; // U+1D11E MUSICAL SYMBOL G CLEF
        match parse_value(src, &mut pos, 0).expect("parses") {
            Json::Str(s) => assert_eq!(s, "\u{1D11E}"),
            other => panic!("expected string, got {other:?}"),
        }
        // A lone surrogate becomes U+FFFD, as in encoding/json.
        let mut pos = 0;
        match parse_value("\"\\ud834\"", &mut pos, 0).expect("parses") {
            Json::Str(s) => assert_eq!(s, "\u{FFFD}"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn number_grammar_is_strict() {
        for bad in ["1.", ".5", "+1", "-", "1e", "1e+", "--1"] {
            let mut pos = 0;
            assert!(
                scan_number(bad, &mut pos).is_err(),
                "'{bad}' should not parse"
            );
        }
        // Leading zeros and hex are caught one level up: `scan_number`
        // consumes the valid `0` prefix and leaves the cursor mid-token,
        // and the enclosing grammar ("expected ',' or '}'" / "trailing
        // data") rejects what follows — same failure `encoding/json`
        // produces.
        for bad in ["01", "0x10", "{\"a\":01}"] {
            assert!(parse_document(bad).is_err(), "'{bad}' should not parse");
        }
        for (good, expected) in [
            ("0", 0.0),
            ("-1.5", -1.5),
            ("1.9e1", 19.0),
            ("4294967295", 4_294_967_295.0),
        ] {
            let mut pos = 0;
            assert_eq!(scan_number(good, &mut pos), Ok(expected), "{good}");
        }
    }

    #[test]
    fn out_of_json_range_numbers_are_value_errors() {
        let event = IngestEvent {
            credentials: decibel_event().credentials,
            row_id: None,
            measurement: Measurement::decibel(1.0, 2.0).expect("finite"),
        };
        let encoded = encode(&event).expect("encodes");
        // Splice a 1e400 dba in — beyond f64, like the Go test's case.
        let body = encoded.body.replace("\"dba\":1", "\"dba\":1e400");
        assert_eq!(
            decode(encoded.path, &body),
            Err(DecodeError::BadValue {
                field: "dba",
                why: "number is out of JSON range",
            })
        );
    }

    #[test]
    fn duplicate_keys_resolve_to_the_last() {
        let decoded = decode(
            "/sensors/pax",
            "{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\
              \"apikey\":\"k\",\"occupancy_count\":1,\"occupancy_count\":19}",
        )
        .expect("decodes");
        assert_eq!(decoded.measurement, Measurement::pax(19),);
    }

    #[test]
    fn depth_is_capped() {
        let deep = format!("{}{}", "[".repeat(64), "]".repeat(64));
        assert!(
            decode("/sensors/pax", &deep).is_err(),
            "deep nesting must be rejected"
        );
    }

    #[test]
    fn trailing_data_is_rejected() {
        assert!(parse_document("{} {}").is_err());
        assert!(parse_document("{}garbage").is_err());
        assert!(parse_document("{}").is_ok());
    }

    #[test]
    fn uuid_alternate_spellings_are_accepted_like_uuid_parse() {
        let uuid = "0A20720E-BF44-4F4C-9A59-D40DD827E7F7";
        assert!(is_uuid(uuid)); // case-insensitive
        assert!(is_uuid("0a20720ebf444f4c9a59d40dd827e7f7")); // naked 32-hex
        assert!(is_uuid("{0a20720e-bf44-4f4c-9a59-d40dd827e7f7}"));
        assert!(is_uuid("urn:uuid:0a20720e-bf44-4f4c-9a59-d40dd827e7f7"));
        assert!(!is_uuid("nope"));
        assert!(!is_uuid("0a20720e-bf44-4f4c-9a59-d40dd827e7fg")); // g
        assert!(!is_uuid("0a20720e-bf44-4f4c-9a59")); // truncated
    }

    #[test]
    fn responses_decode_by_their_single_key() {
        assert_eq!(
            decode_response("{\"status\":\"accepted\"}"),
            Ok(IngestResponse::Status("accepted".into()))
        );
        assert_eq!(
            decode_response("{\"error\":\"dba is required\"}"),
            Ok(IngestResponse::Error("dba is required".into()))
        );
        assert!(decode_response("[1]").is_err());
        assert!(decode_response("{}").is_err());
    }
}
