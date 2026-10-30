//! HTTP-JSON ingest wire codec — the format half of the `transport-http`
//! node.
//!
//! The contract is `server-infra/ingest/service/*.go` in the source
//! monorepo (read-only reference checkout), per the spec's acceptance test
//! "HTTP payload contract-exact": encoder output must be field-for-field
//! against the Go payload structs, and golden tests are derived from the
//! server's own test bodies.
//!
//! # Wire shapes (`POST /v1/sensors/<type>` on the device side — Caddy
//! strips `/v1` and the ingest mux routes `POST /sensors/<type>`; one
//! JSON object per body)
//!
//! Every body carries the flat credentials block
//! (`service.go` `credentials`, promoted by struct embedding):
//! `sensor_id`, `apikey`, `time`. Then per endpoint:
//!
//! | type | fields (Go struct order) | source file |
//! |---|---|---|
//! | `decibel` | `measurement_id`, `dba`, `dbc` | `decibel.go` |
//! | `pax` | `measurement_id`, `occupancy_count` | `pax.go` |
//! | `weather` | `measurement_id`, `temperature`, `humidity`, `precipitation`, `rain_probability`, `wind_speed`, `pressure` | `weather.go` |
//! | `status` | `status_id`, `subsystem`, `result`, `message` | `status.go` |
//!
//! `time` is RFC3339 (the server falls back to now for anything
//! ClickHouse cannot store) and is rendered by the clock side, not here.
//!
//! # What is deliberately NOT encoded
//!
//! - `co2`, `pm25`, `light` have **no HTTP contract** — the server mux
//!   routes only the four types above (and a `/sensors/<type>` for a new
//!   type does not exist until the server grows one). Those measurements
//!   ride the MQTT one-topic-per-type path; [`encode`] refuses them with
//!   [`EncodeError::NoHttpContract`] rather than inventing field names.
//! - Weather's `precipitation`, `rain_probability`, `wind_speed` are
//!   virtual Open-Meteo fields; the spec keeps them device-unset. The
//!   model (and therefore the encoder) carries only the three
//!   device-measurable fields. Note the server today requires all six
//!   weather fields, so a device weather body is expected to be refused
//!   (400) until the server grows device-weather support — the same
//!   grow-into status the new types have.
//!
//! # Stage model
//!
//! [`decode`] models the server's *decode stage* only: `json.Unmarshal`
//! into the payload struct plus the handler's own value checks (required
//! value fields, whole occupancy, the closed `result` vocabulary). Server
//! stages past that — UUID parsing of `sensor_id`, apikey authentication
//! (401), endpoint/type membership (403), row fallbacks for malformed
//! `measurement_id`/`time` — are deliberately not modeled: absent string
//! fields decode as `""` (Go's zero value) and pass through.
//!
//! # Fixed-point numbers, no floats
//!
//! The ring payload is fixed-point (decibel 0.1 dB, temperature 0.1 °C);
//! the wire is JSON numbers. Encoding renders fixed-point exactly
//! (`724` → `72.4`); decoding parses the decimal digits into fixed-point
//! with no float in between, rounding half away from zero at the model's
//! precision. Values the model cannot represent (outside u16 / i16 / u8
//! ranges) are [`DecodeError::OutOfRange`] — the device model is narrower
//! than the server's Float32 columns by design.
//!
//! # Strings
//!
//! The encoder escapes JSON strings properly (`"`, `\`, control bytes).
//! The decoder accepts unescaped UTF-8 only — escape sequences in a
//! captured string are [`DecodeError::EscapedString`]. That covers every
//! conformance fixture (the server's own test bodies are escape-free) and
//! every round-trip of unescaped content; bodies with escapes cannot be
//! produced by this encoder's inputs reaching decode unescaped.
//!
//! Zero heap, no locks, no dependencies: caller-provided byte buffers in,
//! borrowed slices out.

use crate::measurement::{Measurement, result_index, result_name};

/// The ingest types with an HTTP contract — the server mux routes exactly
/// these (`service.go` `New`). The other three [`Measurement`] types ride
/// MQTT.
pub const CONTRACTED_TYPES: [&str; 4] = ["decibel", "pax", "weather", "status"];

/// Sane one-body buffer size for the encoder: the credentials block, a
/// rendered timestamp, an id, and a status message plus slack. Shorter
/// buffers simply get [`EncodeError::BufferFull`]; long status messages
/// need more.
pub const MAX_ENCODED_BODY_LEN: usize = 512;

// ---------------------------------------------------------------------------
// Encode
// ---------------------------------------------------------------------------

/// Why a measurement could not be rendered to the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// The destination buffer cannot hold the full body. Size it from
    /// [`MAX_ENCODED_BODY_LEN`] plus the variable-length strings.
    BufferFull,
    /// The measurement type has no HTTP contract (`co2`, `pm25`, `light`).
    /// They ride the MQTT path — see the module docs.
    NoHttpContract(&'static str),
    /// A `status` measurement was handed a measurement envelope, which
    /// carries no `subsystem` (and no `status_id`).
    MissingSubsystem,
    /// A non-status measurement was handed a status envelope.
    EnvelopeMismatch,
    /// The status `result` index is outside the server's closed
    /// vocabulary — never send an unrenderable result.
    InvalidResult,
}

/// The credentials block every body carries (`service.go` `credentials`).
/// Both values come from the device's own configuration, never from the
/// measurement stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Credentials<'a> {
    /// Wire `sensor_id` — the sensor's UUID.
    pub sensor_id: &'a str,
    /// Wire `apikey` — the sensor's secret (NVS-sourced at runtime).
    pub apikey: &'a str,
}

/// The request envelope around a measurement: credentials, timestamp, and
/// the per-endpoint extras the ring payload deliberately does not carry
/// (strings would make it non-POD). Construct with [`Post::new`] for the
/// decibel/pax/weather endpoints or [`Post::status`] for status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Post<'a> {
    pub creds: Credentials<'a>,
    /// Wire `time` — RFC3339, rendered by the clock side.
    pub time: &'a str,
    inner: PostInner<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostInner<'a> {
    /// `decibel` / `pax` / `weather`: optional client-side
    /// `measurement_id`. Absent is the supported path — the server
    /// generates a UUIDv7 when the field is missing or malformed.
    Measurement { id: Option<&'a str> },
    /// `status`: optional `status_id`, the `subsystem` name (open
    /// vocabulary — config-owned, resolved from the node's index), and an
    /// optional free-form `message`.
    Status {
        id: Option<&'a str>,
        subsystem: &'a str,
        message: Option<&'a str>,
    },
}

impl<'a> Post<'a> {
    /// Envelope for a decibel/pax/weather body.
    pub const fn new(
        creds: Credentials<'a>,
        time: &'a str,
        measurement_id: Option<&'a str>,
    ) -> Self {
        Self {
            creds,
            time,
            inner: PostInner::Measurement { id: measurement_id },
        }
    }

    /// Envelope for a status body.
    pub const fn status(
        creds: Credentials<'a>,
        time: &'a str,
        status_id: Option<&'a str>,
        subsystem: &'a str,
        message: Option<&'a str>,
    ) -> Self {
        Self {
            creds,
            time,
            inner: PostInner::Status {
                id: status_id,
                subsystem,
                message,
            },
        }
    }
}

/// Renders one body into `buf`; returns the byte count written. Field
/// order follows the Go structs: credentials, then the endpoint's own
/// fields in declaration order. Unset optional fields are omitted, not
/// zero-filled — a missing reading must never be recorded as a zero
/// (the server agrees: absent required values are 400, not 0).
pub fn encode(
    buf: &mut [u8],
    post: &Post<'_>,
    m: &Measurement,
) -> Result<usize, EncodeError> {
    let mut w = Writer {
        buf,
        written: 0,
        started: false,
    };
    w.put(b"{")?;
    w.field("sensor_id")?;
    w.string(post.creds.sensor_id)?;
    w.field("apikey")?;
    w.string(post.creds.apikey)?;
    w.field("time")?;
    w.string(post.time)?;

    match m {
        Measurement::Decibel { dba, dbc } => {
            let PostInner::Measurement { id } = &post.inner else {
                return Err(EncodeError::EnvelopeMismatch);
            };
            if let Some(id) = *id {
                w.field("measurement_id")?;
                w.string(id)?;
            }
            w.field("dba")?;
            w.fixed(*dba as i32, 1)?;
            w.field("dbc")?;
            w.fixed(*dbc as i32, 1)?;
        }
        Measurement::Pax { occupancy_count } => {
            let PostInner::Measurement { id } = &post.inner else {
                return Err(EncodeError::EnvelopeMismatch);
            };
            if let Some(id) = *id {
                w.field("measurement_id")?;
                w.string(id)?;
            }
            w.field("occupancy_count")?;
            w.fixed(*occupancy_count as i32, 0)?;
        }
        Measurement::Weather {
            temperature_c,
            humidity_pct,
            pressure_hpa,
        } => {
            let PostInner::Measurement { id } = &post.inner else {
                return Err(EncodeError::EnvelopeMismatch);
            };
            if let Some(id) = *id {
                w.field("measurement_id")?;
                w.string(id)?;
            }
            if let Some(tenth_c) = temperature_c {
                w.field("temperature")?;
                w.fixed(*tenth_c as i32, 1)?;
            }
            if let Some(pct) = humidity_pct {
                w.field("humidity")?;
                w.fixed(*pct as i32, 0)?;
            }
            if let Some(hpa) = pressure_hpa {
                w.field("pressure")?;
                w.fixed(*hpa as i32, 0)?;
            }
        }
        Measurement::Status { result, .. } => {
            let PostInner::Status {
                id,
                subsystem,
                message,
            } = &post.inner
            else {
                return Err(EncodeError::MissingSubsystem);
            };
            // The server answers 400 "subsystem is required" for the empty
            // name — catch it at the source instead of round-tripping a
            // doomed request.
            if subsystem.is_empty() {
                return Err(EncodeError::MissingSubsystem);
            }
            if let Some(id) = *id {
                w.field("status_id")?;
                w.string(id)?;
            }
            w.field("subsystem")?;
            w.string(subsystem)?;
            let Some(name) = result_name(*result) else {
                return Err(EncodeError::InvalidResult);
            };
            w.field("result")?;
            w.string(name)?;
            if let Some(message) = *message {
                w.field("message")?;
                w.string(message)?;
            }
        }
        Measurement::Co2 { .. }
        | Measurement::Pm25 { .. }
        | Measurement::Light { .. } => {
            return Err(EncodeError::NoHttpContract(m.ingest_type()));
        }
    }

    w.put(b"}")?;
    Ok(w.written)
}

/// Byte-sink for one body: appends with capacity checks, tracks field
/// separation, and renders numbers/strings. No heap anywhere.
struct Writer<'b> {
    buf: &'b mut [u8],
    written: usize,
    started: bool,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        let end = self.written + bytes.len();
        if end > self.buf.len() {
            return Err(EncodeError::BufferFull);
        }
        self.buf[self.written..end].copy_from_slice(bytes);
        self.written = end;
        Ok(())
    }

    /// Emits `"name":` (with the separating comma when not first).
    fn field(&mut self, name: &str) -> Result<(), EncodeError> {
        self.put(if self.started { b",\"" } else { b"\"" })?;
        self.started = true;
        self.put(name.as_bytes())?;
        self.put(b"\":")
    }

    /// Emits a JSON string with escapes for `"`, `\` and control bytes.
    /// UTF-8 content passes through byte-for-byte (JSON is UTF-8).
    fn string(&mut self, value: &str) -> Result<(), EncodeError> {
        self.put(b"\"")?;
        for &byte in value.as_bytes() {
            match byte {
                b'"' => self.put(b"\\\"")?,
                b'\\' => self.put(b"\\\\")?,
                0x08 => self.put(b"\\b")?,
                0x0C => self.put(b"\\f")?,
                b'\n' => self.put(b"\\n")?,
                b'\r' => self.put(b"\\r")?,
                b'\t' => self.put(b"\\t")?,
                0x00..=0x1F => {
                    self.put(b"\\u00")?;
                    let hex = b"0123456789abcdef";
                    self.put(&hex[(byte >> 4) as usize..(byte >> 4) as usize + 1])?;
                    self.put(&hex[(byte & 0xF) as usize..(byte & 0xF) as usize + 1])?;
                }
                _ => self.put(&[byte])?,
            }
        }
        self.put(b"\"")
    }

    /// Emits a JSON number for a fixed-point value: `value` units of
    /// 10^-`frac`. `724, 1` → `72.4`; `-105, 1` → `-10.5`; `48, 0` → `48`.
    fn fixed(&mut self, value: i32, frac: u32) -> Result<(), EncodeError> {
        let (negative, magnitude) = if value < 0 {
            (true, value.unsigned_abs())
        } else {
            (false, value as u32)
        };
        if negative {
            self.put(b"-")?;
        }
        if frac == 0 {
            return self.decimal(magnitude);
        }
        let scale = 10u32.pow(frac);
        let whole = magnitude / scale;
        let part = magnitude % scale;
        self.decimal(whole)?;
        self.put(b".")?;
        // The fractional part is zero-padded to exactly `frac` digits.
        let frac = frac as usize;
        let mut digits = [0u8; 8];
        let mut rest = part;
        for position in 0..frac {
            digits[frac - 1 - position] = b'0' + (rest % 10) as u8;
            rest /= 10;
        }
        self.put(&digits[..frac])
    }

    /// Decimal digits of a non-negative value (no zero-padding).
    fn decimal(&mut self, mut value: u32) -> Result<(), EncodeError> {
        let mut digits = [0u8; 10];
        let mut written = 0usize;
        loop {
            digits[9 - written] = b'0' + (value % 10) as u8;
            written += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.put(&digits[10 - written..])
    }
}

// ---------------------------------------------------------------------------
// Decode
// ---------------------------------------------------------------------------

/// Why a body could not be decoded. Payloads are the wire field name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The body is not syntactically valid JSON.
    Malformed,
    /// The top-level value is not an object (arrays are rejected — the
    /// server takes one JSON object per request, never a batch).
    NotAnObject,
    /// Bytes after the closing brace.
    TrailingData,
    /// A known key appeared with the wrong JSON type (the server 400s
    /// these: "a key that appears with the wrong JSON type fails the
    /// decode").
    TypeMismatch(&'static str),
    /// A required value field is absent, named in the server's fixed
    /// check order.
    MissingField(&'static str),
    /// `occupancy_count` had a fractional part (the server requires a
    /// whole number).
    NotWhole(&'static str),
    /// A number cannot be represented in the device model's fixed-point
    /// field (outside u8/u16/i16 after rounding). The server's Float32
    /// columns are wider — this models the device, not the server.
    OutOfRange(&'static str),
    /// The status `result` string is outside the server's closed
    /// vocabulary (the server 400s those).
    InvalidResult,
    /// A captured string contained an escape sequence — outside the
    /// decoder's conformance scope (see module docs).
    EscapedString(&'static str),
    /// The type has no HTTP contract (see [`CONTRACTED_TYPES`]).
    NoHttpContract(&'static str),
    /// An unknown value is nested deeper than the skip cap.
    NestingTooDeep,
}

/// A decoded body: the credentials/time block, the endpoint's optional
/// string fields, and the measurement values in ring-payload form.
///
/// For `status` bodies the open-vocabulary `subsystem` is the string
/// here; [`Measurement::Status`]'s own `subsystem` index is the
/// heartbeat node's registry slot and is not a decode product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoded<'a> {
    /// Wire `sensor_id` — `""` when absent (Go zero value; the server's
    /// UUID check rejects that downstream).
    pub sensor_id: &'a str,
    /// Wire `apikey` — `""` when absent (the server authenticates it).
    pub apikey: &'a str,
    /// Wire `time` — `""` when absent; passed through unvalidated (the
    /// server falls back to now for anything it cannot store).
    pub time: &'a str,
    /// Wire `measurement_id` / `status_id`. An explicitly empty string is
    /// normalized to `None` — the server generates a fresh UUIDv7 for
    /// both, so they are the same outcome.
    pub id: Option<&'a str>,
    /// Wire `subsystem` (status bodies only).
    pub subsystem: Option<&'a str>,
    /// Wire `message` (status bodies only).
    pub message: Option<&'a str>,
    /// The measurement values, in ring-payload form.
    pub measurement: Measurement,
}

/// Decodes one body of the given ingest type (the type arrives on the
/// URL path, not in the body). Strings borrow from `body`; nothing is
/// allocated.
pub fn decode<'a>(body: &'a [u8], ty: &'static str) -> Result<Decoded<'a>, DecodeError> {
    if !CONTRACTED_TYPES.contains(&ty) {
        return Err(DecodeError::NoHttpContract(ty));
    }
    let mut p = Parser { body, pos: 0 };
    p.skip_ws();
    match p.peek() {
        Some(b'{') => p.pos += 1,
        Some(b'[') => return Err(DecodeError::NotAnObject),
        _ => return Err(DecodeError::Malformed),
    }

    let mut caps = Captures::default();
    p.skip_ws();
    if p.peek() == Some(b'}') {
        // An empty object: consume the brace and fall through to the
        // field checks, which report the first missing field.
        p.pos += 1;
    } else {
        loop {
            p.skip_ws();
            if p.peek() != Some(b'"') {
                return Err(DecodeError::Malformed);
            }
            let key = p.scan_key()?;
            p.skip_ws();
            p.expect(b':')?;
            p.skip_ws();
            match key {
                Some(key) => p.capture(&mut caps, ty, key)?,
                // An escaped key cannot match a wire name (the server
                // would unescape it); treat it as unknown.
                None => p.skip_value(0)?,
            }
            p.skip_ws();
            match p.next_byte() {
                Some(b',') => continue,
                Some(b'}') => break,
                _ => return Err(DecodeError::Malformed),
            }
        }
    }
    p.skip_ws();
    if p.pos != p.body.len() {
        return Err(DecodeError::TrailingData);
    }
    build(caps, ty)
}

/// Captured wire fields. `Option` slots implement Go's last-key-wins for
/// duplicates naturally: a later occurrence overwrites the earlier one.
#[derive(Default)]
struct Captures<'a> {
    sensor_id: Option<&'a str>,
    apikey: Option<&'a str>,
    time: Option<&'a str>,
    id: Option<&'a str>,
    subsystem: Option<&'a str>,
    message: Option<&'a str>,
    dba: Option<u16>,
    dbc: Option<u16>,
    occupancy: Option<u8>,
    temperature: Option<i16>,
    humidity: Option<u8>,
    pressure: Option<u16>,
    result: Option<u8>,
}

/// Assembles the [`Decoded`] once parsing succeeded, applying the
/// server's value checks in its fixed order.
fn build<'a>(caps: Captures<'a>, ty: &'static str) -> Result<Decoded<'a>, DecodeError> {
    let measurement = match ty {
        "decibel" => Measurement::Decibel {
            dba: caps.dba.ok_or(DecodeError::MissingField("dba"))?,
            dbc: caps.dbc.ok_or(DecodeError::MissingField("dbc"))?,
        },
        "pax" => Measurement::Pax {
            occupancy_count: caps
                .occupancy
                .ok_or(DecodeError::MissingField("occupancy_count"))?,
        },
        // All weather fields are optional in the device model (SHT4x
        // leaves pressure unset); the server's all-six requirement is a
        // server-side stage — see the module docs.
        "weather" => Measurement::Weather {
            temperature_c: caps.temperature,
            humidity_pct: caps.humidity,
            pressure_hpa: caps.pressure,
        },
        _ => {
            // The server treats an absent and an empty subsystem the
            // same way: 400 "subsystem is required".
            match caps.subsystem {
                Some(name) if !name.is_empty() => (),
                _ => return Err(DecodeError::MissingField("subsystem")),
            }
            Measurement::Status {
                subsystem: 0,
                result: caps.result.ok_or(DecodeError::InvalidResult)?,
            }
        }
    };
    Ok(Decoded {
        sensor_id: caps.sensor_id.unwrap_or(""),
        apikey: caps.apikey.unwrap_or(""),
        time: caps.time.unwrap_or(""),
        id: caps.id,
        subsystem: caps.subsystem,
        message: caps.message,
        measurement,
    })
}

/// A JSON number as its exact decimal parts: ±`mantissa` × 10^`exp10`.
/// No float ever touches the value.
struct RawNumber {
    negative: bool,
    mantissa: u64,
    exp10: i32,
}

impl RawNumber {
    /// Rounds to `frac` fixed-point decimal places, half away from zero.
    fn to_fixed(&self, frac: u32, field: &'static str) -> Result<i32, DecodeError> {
        if self.mantissa == 0 {
            return Ok(0);
        }
        let shift = self.exp10 + frac as i32;
        if shift > 18 {
            return Err(DecodeError::OutOfRange(field));
        }
        if shift >= 0 {
            let scaled = self.mantissa as u128 * 10u128.pow(shift as u32);
            if scaled > i32::MAX as u128 {
                return Err(DecodeError::OutOfRange(field));
            }
            let value = scaled as i32;
            return Ok(if self.negative { -value } else { value });
        }
        let back = (-shift) as u32;
        if back > 19 {
            // mantissa < 10^19 < div/2: the value rounds to zero.
            return Ok(0);
        }
        let div = 10u128.pow(back);
        let mantissa = self.mantissa as u128;
        let q = mantissa / div + u128::from(mantissa % div * 2 >= div);
        if q > i32::MAX as u128 {
            return Err(DecodeError::OutOfRange(field));
        }
        let value = q as i32;
        Ok(if self.negative { -value } else { value })
    }

    /// Rounds to a whole number and range-checks it. Fractional values
    /// are rejected (the server requires a whole `occupancy_count`).
    fn to_whole_within(&self, max: u64, field: &'static str) -> Result<u64, DecodeError> {
        if self.mantissa == 0 {
            return Ok(0); // "-0" is a real zero, like the server's.
        }
        if self.negative {
            return Err(DecodeError::OutOfRange(field));
        }
        if self.exp10 < 0 {
            return Err(DecodeError::NotWhole(field));
        }
        if self.exp10 > 18 {
            return Err(DecodeError::OutOfRange(field));
        }
        let scaled = self.mantissa as u128 * 10u128.pow(self.exp10 as u32);
        if scaled > max as u128 {
            return Err(DecodeError::OutOfRange(field));
        }
        Ok(scaled as u64)
    }
}

/// Cap on unknown-value nesting while skipping (hostile input cannot
/// blow the stack; the real server walks deeper but is not the model).
const MAX_SKIP_DEPTH: usize = 64;

struct Parser<'a> {
    body: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.body.get(self.pos).copied()
    }

    fn next_byte(&mut self) -> Option<u8> {
        let byte = self.peek();
        if byte.is_some() {
            self.pos += 1;
        }
        byte
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, want: u8) -> Result<(), DecodeError> {
        if self.next_byte() == Some(want) {
            Ok(())
        } else {
            Err(DecodeError::Malformed)
        }
    }

    /// Scans a quoted key. Returns `None` for keys containing escapes —
    /// they cannot match a wire name, so the caller skips their value.
    fn scan_key(&mut self) -> Result<Option<&'a str>, DecodeError> {
        self.scan_string().map(|span| span.ok())
    }

    /// Scans a quoted string starting at `"`. `Ok(Err(()))` means the
    /// string contained an escape sequence; the span is consumed either
    /// way.
    fn scan_string(&mut self) -> Result<Result<&'a str, ()>, DecodeError> {
        self.pos += 1; // opening quote (caller checked it)
        let start = self.pos;
        let mut escaped = false;
        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    let text = core::str::from_utf8(&self.body[start..self.pos])
                        .map_err(|_| DecodeError::Malformed)?;
                    self.pos += 1;
                    return Ok(if escaped { Err(()) } else { Ok(text) });
                }
                b'\\' => {
                    escaped = true;
                    self.pos += 2; // the escape and its escaped byte
                }
                _ => self.pos += 1,
            }
        }
        Err(DecodeError::Malformed) // unterminated
    }

    /// A captured string value: must be a string, escape-free.
    fn string_value(&mut self, field: &'static str) -> Result<&'a str, DecodeError> {
        if self.peek() != Some(b'"') {
            return Err(DecodeError::TypeMismatch(field));
        }
        self.scan_string()?
            .ok()
            .ok_or(DecodeError::EscapedString(field))
    }

    /// Consumes a skipped string value, ignoring whether it contained
    /// escapes (unknown values are not captured).
    fn skip_string(&mut self) -> Result<(), DecodeError> {
        self.scan_string().map(|_span| ())
    }

    /// A captured number value: must be a number.
    fn number_value(&mut self, field: &'static str) -> Result<RawNumber, DecodeError> {
        match self.peek() {
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(field),
            _ => Err(DecodeError::TypeMismatch(field)),
        }
    }

    /// Parses a JSON number at the cursor with the strict JSON grammar:
    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
    fn parse_number(&mut self, field: &'static str) -> Result<RawNumber, DecodeError> {
        let negative = self.peek() == Some(b'-');
        if negative {
            self.pos += 1;
        }
        let mut digits_start = self.pos;
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                // No leading zeros: "0" may not be followed by digits.
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(DecodeError::Malformed);
                }
                digits_start = self.pos - 1;
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(DecodeError::Malformed),
        }
        let int_start = digits_start;
        let int_end = self.pos;
        let mut frac: &[u8] = &[];
        let mut exp10: i32 = 0;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            let frac_start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == frac_start {
                return Err(DecodeError::Malformed);
            }
            frac = &self.body[frac_start..self.pos];
            exp10 -= (self.pos - frac_start) as i32;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            let exp_negative = matches!(self.peek(), Some(b'-' | b'+'));
            if exp_negative {
                self.pos += 1;
            }
            let exp_start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == exp_start {
                return Err(DecodeError::Malformed);
            }
            // Saturate: anything past the cap is out of every range the
            // model can hold (or rounds to zero, handled below).
            let mut exponent: i32 = 0;
            for &digit in &self.body[exp_start..self.pos] {
                if exponent <= 1000 {
                    exponent = exponent * 10 + (digit - b'0') as i32;
                }
            }
            exp10 += if exp_negative { -exponent } else { exponent };
        }
        Self::raw_number(negative, &self.body[int_start..int_end], frac, exp10, field)
    }

    /// Turns the integer and fraction digit spans plus the field exponent
    /// into the [`RawNumber`] invariant: sign preserved, trailing zeros
    /// folded into the exponent, at most 19 digits left. The spans hold
    /// digits only — the decimal point separates them in the body.
    fn raw_number(
        negative: bool,
        int: &[u8],
        frac: &[u8],
        mut exp10: i32,
        field: &'static str,
    ) -> Result<RawNumber, DecodeError> {
        // Trailing zeros carry no information: fold them into the
        // exponent, first the fraction's, then (when the fraction is all
        // zeros) the integer's. "3.0" and "3000e-3" both collapse to
        // mantissa 3, exponent 0 — a whole number, like the server sees.
        let mut frac_end = frac.len();
        while frac_end > 0 && frac[frac_end - 1] == b'0' {
            frac_end -= 1;
            exp10 += 1;
        }
        let frac_kept = &frac[..frac_end];
        let mut int_end = int.len();
        if frac_kept.is_empty() {
            while int_end > 1 && int[int_end - 1] == b'0' {
                int_end -= 1;
                exp10 += 1;
            }
        }
        let int_kept = &int[..int_end];
        let total = int_kept.len() + frac_kept.len();
        if total > 19 {
            return Err(DecodeError::OutOfRange(field));
        }
        let mut mantissa: u64 = 0;
        for &digit in int_kept.iter().chain(frac_kept.iter()) {
            mantissa = mantissa * 10 + (digit - b'0') as u64;
        }
        Ok(RawNumber {
            negative,
            mantissa,
            exp10,
        })
    }

    /// Captures one known key's value into the slots, dispatching on the
    /// endpoint type (each endpoint's struct has its own field set — a
    /// `subsystem` key on the decibel endpoint is unknown, like Go's
    /// per-endpoint payload structs).
    fn capture(
        &mut self,
        caps: &mut Captures<'a>,
        ty: &'static str,
        key: &'a str,
    ) -> Result<(), DecodeError> {
        // Go's encoding/json treats an explicit null as the field's zero
        // value: `*float64` pointers become nil (so the handler reports
        // "required"), strings become "". Mirror that by leaving the
        // slot untouched — absent and null decode identically.
        if self.peek() == Some(b'n') {
            return self.expect_literal(b"null");
        }
        match key {
            "sensor_id" => caps.sensor_id = Some(self.string_value("sensor_id")?),
            "apikey" => caps.apikey = Some(self.string_value("apikey")?),
            "time" => caps.time = Some(self.string_value("time")?),
            _ => match (ty, key) {
                ("status", "status_id") => {
                    caps.id = non_empty(self.string_value("status_id")?);
                }
                ("decibel" | "pax" | "weather", "measurement_id") => {
                    caps.id = non_empty(self.string_value("measurement_id")?);
                }
                ("decibel", "dba") => {
                    caps.dba = Some(unsigned_fixed(self.number_value("dba")?, 1, "dba")?);
                }
                ("decibel", "dbc") => {
                    caps.dbc = Some(unsigned_fixed(self.number_value("dbc")?, 1, "dbc")?);
                }
                ("pax", "occupancy_count") => {
                    caps.occupancy = Some(
                        self.number_value("occupancy_count")?
                            .to_whole_within(u8::MAX as u64, "occupancy_count")?
                            as u8,
                    );
                }
                ("weather", "temperature") => {
                    let value = self
                        .number_value("temperature")?
                        .to_fixed(1, "temperature")?;
                    caps.temperature = Some(
                        i16::try_from(value)
                            .map_err(|_| DecodeError::OutOfRange("temperature"))?,
                    );
                }
                ("weather", "humidity") => {
                    let value =
                        unsigned_fixed(self.number_value("humidity")?, 0, "humidity")?;
                    caps.humidity = Some(
                        u8::try_from(value)
                            .map_err(|_| DecodeError::OutOfRange("humidity"))?,
                    );
                }
                ("weather", "pressure") => {
                    caps.pressure = Some(unsigned_fixed(
                        self.number_value("pressure")?,
                        0,
                        "pressure",
                    )?);
                }
                ("status", "subsystem") => {
                    caps.subsystem = Some(self.string_value("subsystem")?);
                }
                ("status", "result") => {
                    let name = self.string_value("result")?;
                    caps.result =
                        Some(result_index(name).ok_or(DecodeError::InvalidResult)?);
                }
                ("status", "message") => {
                    caps.message = Some(self.string_value("message")?);
                }
                _ => self.skip_value(0)?,
            },
        }
        Ok(())
    }

    /// Skips one value of any shape (unknown keys are ignored, like Go's
    /// `encoding/json`). Numbers and literals are fully validated — the
    /// whole body must be valid JSON.
    fn skip_value(&mut self, depth: usize) -> Result<(), DecodeError> {
        if depth > MAX_SKIP_DEPTH {
            return Err(DecodeError::NestingTooDeep);
        }
        self.skip_ws();
        match self.peek().ok_or(DecodeError::Malformed)? {
            b'"' => {
                self.skip_string()?;
            }
            b'-' | b'0'..=b'9' => {
                self.parse_number("unknown")?;
            }
            b't' => self.expect_literal(b"true")?,
            b'f' => self.expect_literal(b"false")?,
            b'n' => self.expect_literal(b"null")?,
            b'{' => {
                self.pos += 1;
                self.skip_ws();
                if self.peek() == Some(b'}') {
                    self.pos += 1;
                } else {
                    loop {
                        self.skip_ws();
                        if self.peek() != Some(b'"') {
                            return Err(DecodeError::Malformed);
                        }
                        self.skip_string()?;
                        self.skip_ws();
                        self.expect(b':')?;
                        self.skip_value(depth + 1)?;
                        self.skip_ws();
                        match self.next_byte() {
                            Some(b',') => continue,
                            Some(b'}') => break,
                            _ => return Err(DecodeError::Malformed),
                        }
                    }
                }
            }
            b'[' => {
                self.pos += 1;
                self.skip_ws();
                if self.peek() == Some(b']') {
                    self.pos += 1;
                } else {
                    loop {
                        self.skip_value(depth + 1)?;
                        self.skip_ws();
                        match self.next_byte() {
                            Some(b',') => continue,
                            Some(b']') => break,
                            _ => return Err(DecodeError::Malformed),
                        }
                    }
                }
            }
            _ => return Err(DecodeError::Malformed),
        }
        Ok(())
    }

    fn expect_literal(&mut self, literal: &[u8]) -> Result<(), DecodeError> {
        if self.body.get(self.pos..self.pos + literal.len()) == Some(literal) {
            self.pos += literal.len();
            Ok(())
        } else {
            Err(DecodeError::Malformed)
        }
    }
}

/// A rounded fixed-point value that must land in an unsigned field.
fn unsigned_fixed(
    number: RawNumber,
    frac: u32,
    field: &'static str,
) -> Result<u16, DecodeError> {
    if number.negative {
        return Err(DecodeError::OutOfRange(field));
    }
    let value = number.to_fixed(frac, field)?;
    u16::try_from(value).map_err(|_| DecodeError::OutOfRange(field))
}

fn non_empty(value: &str) -> Option<&str> {
    if value.is_empty() { None } else { Some(value) }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::{RESULT_DEGRADED, RESULT_FAILED, RESULT_STARTED};

    // The values the ingest service's own test suite uses
    // (service_test.go): reusing them keeps the fixtures and this codec
    // anchored to one set of bodies.
    const SENSOR_ID: &str = "0a20720e-bf44-4f4c-9a59-d40dd827e7f7";
    const API_KEY: &str = "development-api-key";
    const TIME: &str = "2026-08-21T12:00:00Z";
    const MEASUREMENT_ID: &str = "019873f4-1234-7abc-8def-0123456789ab";

    const fn creds() -> Credentials<'static> {
        Credentials {
            sensor_id: SENSOR_ID,
            apikey: API_KEY,
        }
    }

    /// Encodes and asserts the exact wire bytes.
    #[track_caller]
    fn assert_body(post: &Post<'_>, m: &Measurement, want: &str) {
        let mut buf = [0u8; MAX_ENCODED_BODY_LEN];
        let written = encode(&mut buf, post, m).expect("encode");
        assert_eq!(&buf[..written], want.as_bytes());
    }

    #[track_caller]
    fn decode_ok(body: &'static [u8], ty: &'static str) -> Decoded<'static> {
        decode(body, ty).expect("decode")
    }

    // ----- encode: golden bytes -----------------------------------------

    #[test]
    fn encodes_decibel_field_for_field() {
        let post = Post::new(creds(), TIME, Some(MEASUREMENT_ID));
        let m = Measurement::Decibel { dba: 724, dbc: 781 };
        assert_body(
            &post,
            &m,
            "{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\
             \"apikey\":\"development-api-key\",\
             \"time\":\"2026-08-21T12:00:00Z\",\
             \"measurement_id\":\"019873f4-1234-7abc-8def-0123456789ab\",\
             \"dba\":72.4,\"dbc\":78.1}",
        );
    }

    #[test]
    fn encodes_decibel_without_the_optional_id() {
        let post = Post::new(creds(), TIME, None);
        let m = Measurement::Decibel { dba: 0, dbc: 65535 };
        assert_body(
            &post,
            &m,
            "{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\
             \"apikey\":\"development-api-key\",\
             \"time\":\"2026-08-21T12:00:00Z\",\"dba\":0.0,\"dbc\":6553.5}",
        );
    }

    #[test]
    fn encodes_pax_as_a_whole_number() {
        let post = Post::new(creds(), TIME, None);
        let m = Measurement::Pax { occupancy_count: 0 };
        assert_body(
            &post,
            &m,
            "{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\
             \"apikey\":\"development-api-key\",\
             \"time\":\"2026-08-21T12:00:00Z\",\"occupancy_count\":0}",
        );
    }

    #[test]
    fn encodes_only_the_set_weather_fields() {
        let post = Post::new(creds(), TIME, None);
        let m = Measurement::Weather {
            temperature_c: Some(184),
            humidity_pct: Some(71),
            pressure_hpa: None,
        };
        assert_body(
            &post,
            &m,
            "{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\
             \"apikey\":\"development-api-key\",\
             \"time\":\"2026-08-21T12:00:00Z\",\
             \"temperature\":18.4,\"humidity\":71}",
        );
    }

    #[test]
    fn encodes_negative_temperature_with_a_sign() {
        let post = Post::new(creds(), TIME, None);
        let m = Measurement::Weather {
            temperature_c: Some(-52),
            humidity_pct: None,
            pressure_hpa: Some(1018),
        };
        assert_body(
            &post,
            &m,
            "{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\
             \"apikey\":\"development-api-key\",\
             \"time\":\"2026-08-21T12:00:00Z\",\
             \"temperature\":-5.2,\"pressure\":1018}",
        );
    }

    #[test]
    fn encodes_status_without_an_id() {
        let post = Post::status(creds(), TIME, None, "power", Some("battery low"));
        let m = Measurement::Status {
            subsystem: 0,
            result: RESULT_DEGRADED,
        };
        assert_body(
            &post,
            &m,
            "{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\
             \"apikey\":\"development-api-key\",\
             \"time\":\"2026-08-21T12:00:00Z\",\
             \"subsystem\":\"power\",\"result\":\"degraded\",\
             \"message\":\"battery low\"}",
        );
    }

    #[test]
    fn encodes_escapes_and_passes_utf8_through() {
        let post = Post::status(
            creds(),
            TIME,
            None,
            "lifecycle",
            Some("said \"hi\"\u{1}\u{7f}é\nend\\"),
        );
        let m = Measurement::Status {
            subsystem: 0,
            result: RESULT_STARTED,
        };
        assert_body(
            &post,
            &m,
            "{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\
             \"apikey\":\"development-api-key\",\
             \"time\":\"2026-08-21T12:00:00Z\",\
             \"subsystem\":\"lifecycle\",\"result\":\"started\",\
             \"message\":\"said \\\"hi\\\"\\u0001\u{7f}é\\nend\\\\\"}",
        );
    }

    // ----- encode: refusals ---------------------------------------------

    #[test]
    fn refuses_types_without_an_http_contract() {
        let post = Post::new(creds(), TIME, None);
        for (m, ty) in [
            (Measurement::Co2 { ppm: 800 }, "co2"),
            (Measurement::Pm25 { ug_m3: 12 }, "pm25"),
            (Measurement::Light { lux: 100 }, "light"),
        ] {
            let mut buf = [0u8; MAX_ENCODED_BODY_LEN];
            assert_eq!(
                encode(&mut buf, &post, &m),
                Err(EncodeError::NoHttpContract(ty))
            );
        }
    }

    #[test]
    fn refuses_mismatched_envelopes() {
        let mut buf = [0u8; MAX_ENCODED_BODY_LEN];
        // A status body needs the status envelope (subsystem lives there).
        assert_eq!(
            encode(
                &mut buf,
                &Post::new(creds(), TIME, None),
                &Measurement::Status {
                    subsystem: 0,
                    result: 0
                }
            ),
            Err(EncodeError::MissingSubsystem)
        );
        // And a measurement envelope has no subsystem to name.
        assert_eq!(
            encode(
                &mut buf,
                &Post::status(creds(), TIME, None, "power", None),
                &Measurement::Pax { occupancy_count: 3 }
            ),
            Err(EncodeError::EnvelopeMismatch)
        );
        // The server answers 400 for an empty subsystem; catch it here.
        assert_eq!(
            encode(
                &mut buf,
                &Post::status(creds(), TIME, None, "", None),
                &Measurement::Status {
                    subsystem: 0,
                    result: 0
                }
            ),
            Err(EncodeError::MissingSubsystem)
        );
        // An unrenderable result must never reach the wire.
        assert_eq!(
            encode(
                &mut buf,
                &Post::status(creds(), TIME, None, "power", None),
                &Measurement::Status {
                    subsystem: 0,
                    result: 99
                }
            ),
            Err(EncodeError::InvalidResult)
        );
    }

    #[test]
    fn refuses_buffers_that_cannot_hold_the_body() {
        let post = Post::new(creds(), TIME, None);
        let m = Measurement::Pax { occupancy_count: 3 };
        let mut buf = [0u8; 16];
        assert_eq!(encode(&mut buf, &post, &m), Err(EncodeError::BufferFull));
    }

    // ----- round-trips ---------------------------------------------------

    #[test]
    fn round_trips_every_contracted_type() {
        let cases: [(&str, Measurement, Post<'static>); 5] = [
            (
                "decibel",
                Measurement::Decibel { dba: 724, dbc: 781 },
                Post::new(creds(), TIME, Some(MEASUREMENT_ID)),
            ),
            (
                "pax",
                Measurement::Pax {
                    occupancy_count: 19,
                },
                Post::new(creds(), TIME, None),
            ),
            (
                "weather",
                Measurement::Weather {
                    temperature_c: Some(184),
                    humidity_pct: Some(71),
                    pressure_hpa: Some(1018),
                },
                Post::new(creds(), TIME, None),
            ),
            (
                "weather",
                Measurement::Weather {
                    temperature_c: Some(-52),
                    humidity_pct: None,
                    pressure_hpa: None,
                },
                Post::new(creds(), TIME, None),
            ),
            (
                "status",
                Measurement::Status {
                    subsystem: 0,
                    result: RESULT_FAILED,
                },
                Post::status(creds(), TIME, None, "ingest", Some("timeout")),
            ),
        ];
        for (ty, m, post) in cases {
            let mut buf = [0u8; MAX_ENCODED_BODY_LEN];
            let written = encode(&mut buf, &post, &m).expect("encode");
            let decoded = decode(&buf[..written], ty).expect("decode");
            assert_eq!(decoded.measurement, m, "measurement round-trip for {ty}");
            assert_eq!(decoded.sensor_id, SENSOR_ID);
            assert_eq!(decoded.apikey, API_KEY);
            assert_eq!(decoded.time, TIME);
        }
    }

    #[test]
    fn round_trips_whitespace_pretty_printed_bodies() {
        // The server's own validDecibel body is tab-indented with the id
        // between sensor_id and apikey — key order must not matter.
        let body: &'static [u8] = concat!(
            "{\n\t\"sensor_id\": \"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\n",
            "\t\"measurement_id\": \"019873f4-1234-7abc-8def-0123456789ab\",\n",
            "\t\"apikey\": \"development-api-key\",\n",
            "\t\"time\": \"2026-08-21T12:00:00Z\",\n",
            "\t\"dba\": 72.4, \"dbc\": 78.1\n}"
        )
        .as_bytes();
        let decoded = decode_ok(body, "decibel");
        assert_eq!(
            decoded.measurement,
            Measurement::Decibel { dba: 724, dbc: 781 }
        );
        assert_eq!(decoded.id, Some(MEASUREMENT_ID));
    }

    // ----- decode: structure and grammar --------------------------------

    #[test]
    fn rejects_arrays_and_trailing_data_like_the_server() {
        assert_eq!(decode(b"[1,2,3]", "decibel"), Err(DecodeError::NotAnObject));
        assert_eq!(
            decode(b"{\"dba\": 1, \"dbc\": 2} x", "decibel"),
            Err(DecodeError::TrailingData)
        );
        // A truncated body stops at the capture layer (the value that
        // would follow is not a string).
        assert_eq!(
            decode(b"{\"sensor_id\":", "decibel"),
            Err(DecodeError::TypeMismatch("sensor_id"))
        );
        // A missing comma between members is malformed.
        assert_eq!(
            decode(b"{\"sensor_id\":\"s\" \"apikey\":\"k\"}", "decibel"),
            Err(DecodeError::Malformed)
        );
        // An empty body is malformed, not an empty measurement.
        assert_eq!(decode(b"", "decibel"), Err(DecodeError::Malformed));
    }

    #[test]
    fn rejects_wrong_json_types_with_the_field_name() {
        // Bodies mirrored from the server's own type-mismatch tests.
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"0a20720e-bf44-4f4c-9a59-d40dd827e7f7\",\"apikey\":\"k\",\"dba\":\"72.4\",\"dbc\":78.1}",
                "decibel"
            ),
            Err(DecodeError::TypeMismatch("dba"))
        );
        assert_eq!(
            decode(
                b"{\"sensor_id\":5,\"apikey\":\"k\",\"dba\":1,\"dbc\":2}",
                "decibel"
            ),
            Err(DecodeError::TypeMismatch("sensor_id"))
        );
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"occupancy_count\":\"19\"}",
                "pax"
            ),
            Err(DecodeError::TypeMismatch("occupancy_count"))
        );
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"subsystem\":\"x\",\"result\":5}",
                "status"
            ),
            Err(DecodeError::TypeMismatch("result"))
        );
    }

    #[test]
    fn rejects_missing_required_fields_in_server_order() {
        // The server's "absent dba" body — dba is named before dbc.
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"dbc\":78.1}",
                "decibel"
            ),
            Err(DecodeError::MissingField("dba"))
        );
        assert_eq!(
            decode(b"{}", "decibel"),
            Err(DecodeError::MissingField("dba"))
        );
        assert_eq!(
            decode(b"{\"dba\":1}", "decibel"),
            Err(DecodeError::MissingField("dbc"))
        );
        assert_eq!(
            decode(b"{\"sensor_id\":\"s\",\"apikey\":\"k\"}", "pax"),
            Err(DecodeError::MissingField("occupancy_count"))
        );
        // The server treats an absent and an empty subsystem the same.
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"result\":\"failed\"}",
                "status"
            ),
            Err(DecodeError::MissingField("subsystem"))
        );
        assert_eq!(
            decode(b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"subsystem\":\"\",\"result\":\"failed\"}", "status"),
            Err(DecodeError::MissingField("subsystem"))
        );
    }

    #[test]
    fn enforces_the_status_result_vocabulary() {
        // A result outside the closed vocabulary is a 400 server-side.
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"subsystem\":\"upstream\",\"result\":\"ok\"}",
                "status"
            ),
            Err(DecodeError::InvalidResult)
        );
        // And so is an absent one (the empty string is not in the list).
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"subsystem\":\"upstream\"}",
                "status"
            ),
            Err(DecodeError::InvalidResult)
        );
        for (name, expected) in [
            ("started", 0),
            ("stopped", 1),
            ("success", 2),
            ("degraded", 3),
            ("failed", 4),
        ] {
            let body: &'static [u8] = Box::leak(
                format!(
                    "{{\"sensor_id\":\"s\",\"apikey\":\"k\",\"subsystem\":\"upstream\",\"result\":\"{name}\"}}"
                )
                .into_boxed_str()
                .into_boxed_bytes(),
            );
            let decoded = decode_ok(body, "status");
            let Measurement::Status { result, .. } = decoded.measurement else {
                panic!("expected a status measurement");
            };
            assert_eq!(result, expected, "result index for {name}");
        }
    }

    #[test]
    fn rejects_numbers_the_grammar_forbids() {
        for bad in [
            // Leading zeros, dangling separators, bare exponents.
            "\"dba\":01,\"dbc\":2",
            "\"dba\":1.,\"dbc\":2",
            "\"dba\":1e,\"dbc\":2",
            "\"dba\":1e+,\"dbc\":2",
            "\"dba\":-,\"dbc\":2",
            "\"dba\":+1,\"dbc\":2",
            "\"dba\":NaN,\"dbc\":2",
            "\"dba\":Infinity,\"dbc\":2",
        ] {
            let body: &'static [u8] = Box::leak(
                format!("{{\"sensor_id\":\"s\",\"apikey\":\"k\",{bad} }}")
                    .into_boxed_str()
                    .into_boxed_bytes(),
            );
            let result = decode(body, "decibel");
            assert!(
                matches!(
                    result,
                    Err(DecodeError::Malformed | DecodeError::TypeMismatch(_))
                ),
                "{bad}: {result:?}"
            );
        }
    }

    #[test]
    fn models_the_device_field_ranges_not_the_server_columns() {
        // The server's Float32 columns are wider; the device model is
        // u16 0.1 dB / i16 0.1 °C / u8 %. These are 400s on either side
        // or 400s the server would accept — both decode errors.
        for (ty, body, err) in [
            (
                "decibel",
                "\"dba\":-1,\"dbc\":2",
                DecodeError::OutOfRange("dba"),
            ),
            (
                "decibel",
                "\"dba\":6553.6,\"dbc\":2",
                DecodeError::OutOfRange("dba"),
            ),
            (
                "decibel",
                "\"dba\":1e300,\"dbc\":2",
                DecodeError::OutOfRange("dba"),
            ),
            (
                "decibel",
                "\"dba\":1e400,\"dbc\":2",
                DecodeError::OutOfRange("dba"),
            ),
            (
                "pax",
                "\"occupancy_count\":-1",
                DecodeError::OutOfRange("occupancy_count"),
            ),
            (
                "pax",
                "\"occupancy_count\":256",
                DecodeError::OutOfRange("occupancy_count"),
            ),
            (
                "pax",
                "\"occupancy_count\":4294967296",
                DecodeError::OutOfRange("occupancy_count"),
            ),
            (
                "weather",
                "\"temperature\":-3276.9",
                DecodeError::OutOfRange("temperature"),
            ),
        ] {
            let body: &'static [u8] = Box::leak(
                format!("{{\"sensor_id\":\"s\",\"apikey\":\"k\",{body}}}")
                    .into_boxed_str()
                    .into_boxed_bytes(),
            );
            assert_eq!(
                decode(body, ty),
                Err(err),
                "body: {}",
                core::str::from_utf8(body).unwrap_or("?")
            );
        }
    }

    #[test]
    fn requires_whole_occupancy_counts() {
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"occupancy_count\":3.5}",
                "pax"
            ),
            Err(DecodeError::NotWhole("occupancy_count"))
        );
        // Exact representations of whole numbers are fine.
        for (body, expected) in [
            ("\"occupancy_count\":3.0", 3),
            ("\"occupancy_count\":3000e-3", 3),
            ("\"occupancy_count\":2e1", 20),
            ("\"occupancy_count\":0", 0),
        ] {
            let full: &'static [u8] = Box::leak(
                format!("{{\"sensor_id\":\"s\",\"apikey\":\"k\",{body}}}")
                    .into_boxed_str()
                    .into_boxed_bytes(),
            );
            let decoded = decode_ok(full, "pax");
            assert_eq!(
                decoded.measurement,
                Measurement::Pax {
                    occupancy_count: expected
                },
                "body: {body}"
            );
        }
    }

    #[test]
    fn rounds_half_away_from_zero_at_the_model_precision() {
        let body: &'static [u8] =
            b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"dba\":72.45,\"dbc\":0.05}";
        let decoded = decode_ok(body, "decibel");
        assert_eq!(
            decoded.measurement,
            Measurement::Decibel {
                dba: 725, // 72.45 → 72.5
                dbc: 1,   // 0.05 → 0.1
            }
        );
    }

    #[test]
    fn decodes_weather_values_into_fixed_point() {
        // Mirrors the server's validWeather readings for the three
        // device-measurable fields.
        let body: &'static [u8] = concat!(
            "{",
            "\"sensor_id\":\"630d11c4-57d8-4a72-9899-3eb995f6e147\",",
            "\"apikey\":\"development-api-key\",",
            "\"time\":\"2026-08-21T12:00:00Z\",",
            "\"temperature\":18.4,\"humidity\":71,\"precipitation\":0.2,",
            "\"rain_probability\":40,\"wind_speed\":12.7,\"pressure\":1018.4}"
        )
        .as_bytes();
        let decoded = decode_ok(body, "weather");
        assert_eq!(
            decoded.measurement,
            Measurement::Weather {
                temperature_c: Some(184),
                humidity_pct: Some(71),
                pressure_hpa: Some(1018), // 1018.4 rounds to 1018
            }
        );
        assert_eq!(decoded.sensor_id, "630d11c4-57d8-4a72-9899-3eb995f6e147");
    }

    #[test]
    fn skips_unknown_keys_including_nested_values() {
        // Mirrors the server's TestPayloadCannotOverrideVenueOrLocation:
        // venue_id/location/unexpected_key are ignored, whatever their shape.
        let body: &'static [u8] = concat!(
            "{",
            "\"sensor_id\":\"630d11c4-57d8-4a72-9899-3eb995f6e147\",",
            "\"apikey\":\"development-api-key\",",
            "\"venue_id\":\"ffffffff-ffff-ffff-ffff-ffffffffffff\",",
            "\"location\":\"Somewhere else\",",
            "\"unexpected_key\":{\"nested\":[1,2,3,null,true,false]},",
            "\"temperature\":1,\"humidity\":2,\"precipitation\":3,",
            "\"rain_probability\":4,\"wind_speed\":5,\"pressure\":6}"
        )
        .as_bytes();
        let decoded = decode_ok(body, "weather");
        assert_eq!(
            decoded.measurement,
            Measurement::Weather {
                temperature_c: Some(10),
                humidity_pct: Some(2),
                pressure_hpa: Some(6),
            }
        );
        // A status key on the decibel endpoint is just unknown, like Go's
        // per-endpoint payload structs.
        let decibel_with_subsystem: &'static [u8] =
            b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"subsystem\":\"x\",\"result\":\"failed\",\"dba\":1,\"dbc\":2}";
        let decoded = decode_ok(decibel_with_subsystem, "decibel");
        assert_eq!(decoded.subsystem, None);
        assert_eq!(
            decoded.measurement,
            Measurement::Decibel { dba: 10, dbc: 20 }
        );
    }

    #[test]
    fn last_duplicate_key_wins() {
        let body: &'static [u8] =
            b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"dba\":1,\"dba\":22.5,\"dbc\":2}";
        let decoded = decode_ok(body, "decibel");
        assert_eq!(
            decoded.measurement,
            Measurement::Decibel { dba: 225, dbc: 20 }
        );
    }

    #[test]
    fn treats_null_as_the_go_zero_value() {
        // encoding/json: null leaves a *float64 nil (→ "required") and a
        // string "" — decode mirrors both by treating null as absent.
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"dba\":null,\"dbc\":2}",
                "decibel"
            ),
            Err(DecodeError::MissingField("dba"))
        );
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"subsystem\":null,\"result\":null}",
                "status"
            ),
            Err(DecodeError::MissingField("subsystem"))
        );
        let body: &'static [u8] =
            b"{\"sensor_id\":null,\"apikey\":null,\"measurement_id\":null,\"dba\":1,\"dbc\":2}";
        let decoded = decode_ok(body, "decibel");
        assert_eq!(decoded.sensor_id, "");
        assert_eq!(decoded.apikey, "");
        assert_eq!(decoded.time, "");
        assert_eq!(decoded.id, None);
    }

    #[test]
    fn refuses_escaped_strings_in_captured_fields() {
        // The server's Unmarshal would unescape these; decode's
        // conformance scope stops at escape-free UTF-8.
        assert_eq!(
            decode(
                b"{\"sensor_id\":\"s\",\"apikey\":\"k\",\"subsystem\":\"upstream\",\"result\":\"failed\",\"message\":\"a\\u0041b\"}",
                "status"
            ),
            Err(DecodeError::EscapedString("message"))
        );
        // An escaped KEY cannot match a wire name, so it is unknown and
        // its value is skipped, like the server's unknown fields.
        let body: &'static [u8] =
            b"{\"sensor_id\":\"s\",\"\\u0061pikey\":\"evil\",\"dba\":1,\"dbc\":2}";
        let decoded = decode_ok(body, "decibel");
        assert_eq!(decoded.apikey, "");
    }

    #[test]
    fn refuses_decoding_types_without_an_http_contract() {
        assert_eq!(
            decode(b"{}", "co2"),
            Err(DecodeError::NoHttpContract("co2"))
        );
    }
}
