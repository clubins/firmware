//! HTTP JSON codec — encoder and decoder for the ingest bodies, gated by
//! `transport-http`.
//!
//! The wire contract is the backend's `server-infra/ingest/service/*.go`:
//! flat JSON objects, `credentials` promoted into every payload, unknown keys
//! ignored, wrong-typed keys rejected, values validated exactly as the
//! service validates them (`measurementValue`, `occupancyCount`,
//! `knownResults`, `uuid.Parse` on `sensor_id`). Field names, check order and
//! the [`DecodeError`] messages follow the Go source; a rejected body reads
//! the same on both sides of the contract.
//!
//! Two families of types mirror the two contract layers:
//!
//! * device emissions use the validated model types ([`crate::measurement`]);
//!   the encoders write them into a caller-provided buffer — allocation-free,
//!   in the Go structs' field order. A 1 KiB buffer holds any conforming
//!   body (the service's own cap is 1 MiB).
//! * the decoders return the *payload* types with [`Cow`] strings borrowed
//!   from the input — the server-faithful view that accepts exactly what the
//!   service accepts (empty `apikey`, absent `time`, `null` values) — used
//!   for round-trip proofs and the contract-conformance fixtures.
//!
//! Known divergences from `encoding/json`, all narrower: escaped JSON *keys*
//! never match a known field name (no legitimate sender escapes fixed keys),
//! and the object-nesting skip is depth-bounded at [`MAX_DEPTH`] (the service
//! allows 10 000) as a parser-DoS guard.

use alloc::borrow::Cow;
use alloc::string::String;
use core::fmt;
use core::str::Chars;

use crate::measurement::{
    DecibelMeasurement, PaxMeasurement, StatusMeasurement, StatusResult,
    WeatherMeasurement, is_uuid,
};

/// Encoding failure. Values are pre-validated by the model, so only two
/// things can go wrong: the caller-sized buffer, or a hand-built value that
/// skipped `new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The target buffer cannot hold the body; nothing was written
    /// completely, so a partial buffer must not be sent.
    Overflow,
    /// A measurement value is NaN or infinite (defence in depth — the model
    /// constructors reject these already).
    NotFinite(&'static str),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("buffer too small for the JSON body"),
            Self::NotFinite(name) => write!(f, "{name} must be finite"),
        }
    }
}

impl core::error::Error for EncodeError {}

/// Decode failure, in the service's own vocabulary. The json.Unmarshal class
/// (Syntax, WrongType) is what service.go:161 answers as `invalid request
/// body: <json detail>`; the rest quote the service's 400 strings verbatim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not one valid JSON object, or trailing data after it.
    Syntax,
    /// A known key arrived with the wrong JSON type.
    WrongType(&'static str),
    /// `sensor_id` is not a UUID (`uuid.Parse` in the authorize stage).
    SensorIdNotUuid,
    /// A required value was absent or `null` (`measurementValue`).
    Required(&'static str),
    /// A measurement value is not a finite 32-bit float (`measurementValue`).
    NotFinite(&'static str),
    /// `occupancy_count` is not a finite number (`occupancyCount`).
    OccupancyNotFinite,
    /// `occupancy_count` is outside the UInt32 column's range.
    OccupancyRange,
    /// `occupancy_count` is fractional.
    OccupancyNotWhole,
    /// `subsystem` is empty (`handleStatus`).
    SubsystemEmpty,
    /// `result` is outside the closed vocabulary (`handleStatus`).
    ResultInvalid,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax | Self::WrongType(_) => f.write_str("invalid request body"),
            Self::SensorIdNotUuid => f.write_str("sensor_id must be a UUID"),
            Self::Required(name) => write!(f, "{name} is required"),
            Self::NotFinite(name) => write!(f, "{name} must be a finite 32-bit float"),
            Self::OccupancyNotFinite => {
                f.write_str("occupancy_count must be a finite number")
            }
            Self::OccupancyRange => {
                f.write_str("occupancy_count must be between 0 and 4294967295")
            }
            Self::OccupancyNotWhole => {
                f.write_str("occupancy_count must be a whole number")
            }
            Self::SubsystemEmpty => f.write_str("subsystem is required"),
            Self::ResultInvalid => f.write_str(
                "result must be one of [started stopped success degraded failed]",
            ),
        }
    }
}

impl core::error::Error for DecodeError {}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Formats one number into a fixed scratch buffer before it is copied into
/// the target — `core::fmt` writes through a trait, not into slices directly.
struct NumBuf {
    len: usize,
    bytes: [u8; 48],
}

impl NumBuf {
    fn new() -> Self {
        Self {
            len: 0,
            bytes: [0; 48],
        }
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl fmt::Write for NumBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        if end > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Writes JSON into the caller's buffer. Overflow is sticky: writing
/// continues (dropping bytes) so the failure surfaces once in [`finish`],
/// never as a silently truncated length.
struct Writer<'a> {
    buf: &'a mut [u8],
    len: usize,
    overflow: bool,
}

impl<'a> Writer<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            len: 0,
            overflow: false,
        }
    }

    fn raw(&mut self, bytes: &[u8]) {
        let end = self.len + bytes.len();
        if end <= self.buf.len() {
            self.buf[self.len..end].copy_from_slice(bytes);
            self.len = end;
        } else {
            self.overflow = true;
        }
    }

    fn key(&mut self, name: &str) {
        self.raw(b"\"");
        self.raw(name.as_bytes());
        self.raw(b"\":");
    }

    /// One JSON string: RFC 8259 minimal escaping — the two mandatory
    /// escapes, the short forms for the three whitespace controls, and
    /// `\u00xx` for other control characters. Non-ASCII passes through as
    /// UTF-8, which is what the service's decoder reads.
    fn string(&mut self, s: &str) {
        self.raw(b"\"");
        for ch in s.chars() {
            match ch {
                '"' => self.raw(b"\\\""),
                '\\' => self.raw(b"\\\\"),
                '\n' => self.raw(b"\\n"),
                '\r' => self.raw(b"\\r"),
                '\t' => self.raw(b"\\t"),
                c if (c as u32) < 0x20 => {
                    let hex = [
                        b'\\',
                        b'u',
                        b'0',
                        b'0',
                        HEX_DIGITS[((c as u32) >> 4) as usize],
                        HEX_DIGITS[(c as u32 & 0xf) as usize],
                    ];
                    self.raw(&hex);
                }
                c => {
                    let mut utf8 = [0u8; 4];
                    self.raw(c.encode_utf8(&mut utf8).as_bytes());
                }
            }
        }
        self.raw(b"\"");
    }

    fn pair_str(&mut self, name: &str, value: &str) {
        self.key(name);
        self.string(value);
    }

    /// f32 values go out as their f64 decimal — what a Go device does by
    /// casting to the `*float64` struct fields before `json.Marshal`. The
    /// f32 shortest form of values near `f32::MAX` (`34028235…`) exceeds
    /// `MaxFloat32` once re-parsed as f64, and the service would reject the
    /// body we just built; the f64 form round-trips exactly.
    fn pair_f32(&mut self, name: &str, value: f32) {
        self.key(name);
        let mut num = NumBuf::new();
        match fmt::write(&mut num, format_args!("{}", value as f64)) {
            Ok(()) => self.raw(num.as_slice()),
            Err(_) => self.overflow = true,
        }
    }

    fn pair_u32(&mut self, name: &str, value: u32) {
        self.key(name);
        let mut num = NumBuf::new();
        match fmt::write(&mut num, format_args!("{}", value)) {
            Ok(()) => self.raw(num.as_slice()),
            Err(_) => self.overflow = true,
        }
    }

    fn finish(self) -> Result<usize, EncodeError> {
        if self.overflow {
            Err(EncodeError::Overflow)
        } else {
            Ok(self.len)
        }
    }
}

fn check_finite(name: &'static str, values: &[f32]) -> Result<(), EncodeError> {
    for value in values {
        if !value.is_finite() {
            return Err(EncodeError::NotFinite(name));
        }
    }
    Ok(())
}

/// Encodes one `/sensors/decibel` body into `buf`, returning the length
/// written. Field order is `decibelPayload`'s.
pub fn encode_decibel(
    m: &DecibelMeasurement,
    buf: &mut [u8],
) -> Result<usize, EncodeError> {
    check_finite("dba", &[m.dba])?;
    check_finite("dbc", &[m.dbc])?;
    let mut w = Writer::new(buf);
    w.raw(b"{");
    w.pair_str("sensor_id", m.credentials.sensor_id);
    w.raw(b",");
    w.pair_str("apikey", m.credentials.apikey);
    w.raw(b",");
    w.pair_str("time", m.credentials.time);
    w.raw(b",");
    w.pair_str("measurement_id", m.measurement_id);
    w.raw(b",");
    w.pair_f32("dba", m.dba);
    w.raw(b",");
    w.pair_f32("dbc", m.dbc);
    w.raw(b"}");
    w.finish()
}

/// Encodes one `/sensors/pax` body into `buf`. Field order is `paxPayload`'s.
pub fn encode_pax(m: &PaxMeasurement, buf: &mut [u8]) -> Result<usize, EncodeError> {
    let mut w = Writer::new(buf);
    w.raw(b"{");
    w.pair_str("sensor_id", m.credentials.sensor_id);
    w.raw(b",");
    w.pair_str("apikey", m.credentials.apikey);
    w.raw(b",");
    w.pair_str("time", m.credentials.time);
    w.raw(b",");
    w.pair_str("measurement_id", m.measurement_id);
    w.raw(b",");
    w.pair_u32("occupancy_count", m.occupancy_count);
    w.raw(b"}");
    w.finish()
}

/// Encodes one `/sensors/weather` body into `buf`. Field order is
/// `weatherPayload`'s — credentials, id, then the six `m_weather` columns.
pub fn encode_weather(
    m: &WeatherMeasurement,
    buf: &mut [u8],
) -> Result<usize, EncodeError> {
    check_finite(
        "temperature",
        &[
            m.values.temperature,
            m.values.humidity,
            m.values.precipitation,
        ],
    )?;
    check_finite(
        "rain_probability",
        &[
            m.values.rain_probability,
            m.values.wind_speed,
            m.values.pressure,
        ],
    )?;
    let mut w = Writer::new(buf);
    w.raw(b"{");
    w.pair_str("sensor_id", m.credentials.sensor_id);
    w.raw(b",");
    w.pair_str("apikey", m.credentials.apikey);
    w.raw(b",");
    w.pair_str("time", m.credentials.time);
    w.raw(b",");
    w.pair_str("measurement_id", m.measurement_id);
    w.raw(b",");
    w.pair_f32("temperature", m.values.temperature);
    w.raw(b",");
    w.pair_f32("humidity", m.values.humidity);
    w.raw(b",");
    w.pair_f32("precipitation", m.values.precipitation);
    w.raw(b",");
    w.pair_f32("rain_probability", m.values.rain_probability);
    w.raw(b",");
    w.pair_f32("wind_speed", m.values.wind_speed);
    w.raw(b",");
    w.pair_f32("pressure", m.values.pressure);
    w.raw(b"}");
    w.finish()
}

/// Encodes one `/sensors/status` body into `buf`. Field order is
/// `statusPayload`'s.
pub fn encode_status(
    m: &StatusMeasurement,
    buf: &mut [u8],
) -> Result<usize, EncodeError> {
    let mut w = Writer::new(buf);
    w.raw(b"{");
    w.pair_str("sensor_id", m.credentials.sensor_id);
    w.raw(b",");
    w.pair_str("apikey", m.credentials.apikey);
    w.raw(b",");
    w.pair_str("time", m.credentials.time);
    w.raw(b",");
    w.pair_str("status_id", m.status_id);
    w.raw(b",");
    w.pair_str("subsystem", m.subsystem);
    w.raw(b",");
    w.pair_str("result", m.result.as_str());
    w.raw(b",");
    w.pair_str("message", m.message);
    w.raw(b"}");
    w.finish()
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

const MAX_DEPTH: usize = 128;

struct Scanner<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, pos: 0 }
    }

    fn skip_ws(&mut self) {
        while let Some(&b) = self.s.as_bytes().get(self.pos) {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }

    fn expect(&mut self, b: u8) -> Result<(), DecodeError> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(DecodeError::Syntax)
        }
    }

    /// Go rejects anything after the top-level value.
    fn at_end(&mut self) -> Result<(), DecodeError> {
        self.skip_ws();
        if self.pos == self.s.len() {
            Ok(())
        } else {
            Err(DecodeError::Syntax)
        }
    }

    /// Walks one flat JSON object, handing each key with its raw value span
    /// to `f`. Values are skipped in full (syntax-checked, depth-bounded), so
    /// unknown keys of any shape are tolerated exactly as `encoding/json`
    /// skips them; `f` interprets the spans it knows. Duplicate keys re-enter
    /// `f` — the service's "last one wins" falls out of the assignment order.
    fn fields<F>(&mut self, mut f: F) -> Result<(), DecodeError>
    where
        F: FnMut(&'a str, &'a str) -> Result<(), DecodeError>,
    {
        self.skip_ws();
        self.expect(b'{')?;
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(());
        }
        loop {
            self.skip_ws();
            let key = self.string_span()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.value_span()?;
            f(key, value)?;
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => return Err(DecodeError::Syntax),
            }
        }
    }

    /// Content span of one JSON string token (input starts at `"`); escapes
    /// are validated but left raw — interpreters decide whether to decode.
    fn string_span(&mut self) -> Result<&'a str, DecodeError> {
        self.expect(b'"')?;
        let start = self.pos;
        loop {
            let Some(b) = self.peek() else {
                return Err(DecodeError::Syntax);
            };
            self.pos += 1;
            match b {
                b'"' => {
                    return self.s.get(start..self.pos - 1).ok_or(DecodeError::Syntax);
                }
                b'\\' => self.skip_escape()?,
                0x00..=0x1f => return Err(DecodeError::Syntax),
                _ => {}
            }
        }
    }

    fn skip_escape(&mut self) -> Result<(), DecodeError> {
        let Some(b) = self.peek() else {
            return Err(DecodeError::Syntax);
        };
        self.pos += 1;
        match b {
            b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => Ok(()),
            b'u' => {
                for _ in 0..4 {
                    let Some(h) = self.peek() else {
                        return Err(DecodeError::Syntax);
                    };
                    if !h.is_ascii_hexdigit() {
                        return Err(DecodeError::Syntax);
                    }
                    self.pos += 1;
                }
                Ok(())
            }
            _ => Err(DecodeError::Syntax),
        }
    }

    /// Raw span of one JSON value of any shape.
    fn value_span(&mut self) -> Result<&'a str, DecodeError> {
        let start = self.pos;
        self.skip_value(0)?;
        self.s.get(start..self.pos).ok_or(DecodeError::Syntax)
    }

    fn skip_value(&mut self, depth: usize) -> Result<(), DecodeError> {
        if depth > MAX_DEPTH {
            return Err(DecodeError::Syntax);
        }
        match self.peek() {
            Some(b'"') => {
                self.string_span()?;
                Ok(())
            }
            Some(b'-') | Some(b'0'..=b'9') => {
                self.number_span()?;
                Ok(())
            }
            Some(b'{') => {
                self.pos += 1;
                self.skip_ws();
                if self.peek() == Some(b'}') {
                    self.pos += 1;
                    return Ok(());
                }
                loop {
                    self.skip_ws();
                    self.string_span()?;
                    self.skip_ws();
                    self.expect(b':')?;
                    self.skip_ws();
                    self.skip_value(depth + 1)?;
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(());
                        }
                        _ => return Err(DecodeError::Syntax),
                    }
                }
            }
            Some(b'[') => {
                self.pos += 1;
                self.skip_ws();
                if self.peek() == Some(b']') {
                    self.pos += 1;
                    return Ok(());
                }
                loop {
                    self.skip_ws();
                    self.skip_value(depth + 1)?;
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(());
                        }
                        _ => return Err(DecodeError::Syntax),
                    }
                }
            }
            Some(b't') => self.literal("true"),
            Some(b'f') => self.literal("false"),
            Some(b'n') => self.literal("null"),
            _ => Err(DecodeError::Syntax),
        }
    }

    fn literal(&mut self, word: &str) -> Result<(), DecodeError> {
        if self
            .s
            .get(self.pos..)
            .is_some_and(|rest| rest.starts_with(word))
        {
            self.pos += word.len();
            Ok(())
        } else {
            Err(DecodeError::Syntax)
        }
    }

    /// Validates one RFC 8259 number token and returns its span. `encoding/json`
    /// rejects malformed numbers as syntax errors, and so does this.
    fn number_span(&mut self) -> Result<&'a str, DecodeError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(DecodeError::Syntax),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(DecodeError::Syntax);
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(DecodeError::Syntax);
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        self.s.get(start..self.pos).ok_or(DecodeError::Syntax)
    }
}

/// A string value as the service decodes it: `null` is the zero value,
/// anything but a JSON string is a type error, and escapes decode to UTF-8.
fn string_field<'a>(
    name: &'static str,
    raw: &'a str,
) -> Result<Cow<'a, str>, DecodeError> {
    if raw == "null" {
        return Ok(Cow::Borrowed(""));
    }
    let content = match (raw.as_bytes().first(), raw.as_bytes().last()) {
        (Some(b'"'), Some(b'"')) if raw.len() >= 2 => &raw[1..raw.len() - 1],
        _ => return Err(DecodeError::WrongType(name)),
    };
    if content.contains('\\') {
        unescape(content).map(Cow::Owned)
    } else {
        Ok(Cow::Borrowed(content))
    }
}

fn unescape(s: &str) -> Result<String, DecodeError> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('u') => unescape_hex(&mut chars, &mut out)?,
                _ => return Err(DecodeError::Syntax),
            },
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// Decodes one `\uXXXX` escape into `out`, combining surrogate pairs.
/// Unpaired halves become U+FFFD, as `encoding/json` produces.
fn unescape_hex(chars: &mut Chars<'_>, out: &mut String) -> Result<(), DecodeError> {
    let hi = hex4(chars)?;
    if (0xD800..=0xDBFF).contains(&hi) {
        // High surrogate: a low half must follow as a second \uXXXX.
        let restore = chars.clone();
        let paired = chars.next() == Some('\\') && chars.next() == Some('u');
        if paired {
            let lo = hex4(chars)?;
            match lo {
                0xDC00..=0xDFFF => {
                    let combined = 0x1_0000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                    out.push(char::from_u32(combined).ok_or(DecodeError::Syntax)?);
                }
                0xD800..=0xDBFF => {
                    out.push('\u{FFFD}');
                    out.push('\u{FFFD}');
                }
                _ => {
                    out.push('\u{FFFD}');
                    out.push(char::from_u32(lo).ok_or(DecodeError::Syntax)?);
                }
            }
        } else {
            *chars = restore;
            out.push('\u{FFFD}');
        }
    } else if (0xDC00..=0xDFFF).contains(&hi) {
        out.push('\u{FFFD}');
    } else {
        out.push(char::from_u32(hi).ok_or(DecodeError::Syntax)?);
    }
    Ok(())
}

fn hex4(chars: &mut Chars<'_>) -> Result<u32, DecodeError> {
    let mut value = 0;
    for _ in 0..4 {
        let Some(c) = chars.next() else {
            return Err(DecodeError::Syntax);
        };
        let Some(digit) = c.to_digit(16) else {
            return Err(DecodeError::Syntax);
        };
        value = value * 16 + digit;
    }
    Ok(value)
}

/// A numeric value as the service decodes it: `null` is absent (the pointer
/// stays nil and the endpoint reports it required), anything but a JSON
/// number is a type error.
fn number_field(name: &'static str, raw: &str) -> Result<Option<f64>, DecodeError> {
    if raw == "null" {
        return Ok(None);
    }
    match raw.as_bytes().first() {
        Some(b'-') | Some(b'0'..=b'9') => {
            let value: f64 = raw.parse().map_err(|_| DecodeError::Syntax)?;
            Ok(Some(value))
        }
        _ => Err(DecodeError::WrongType(name)),
    }
}

/// `measurementValue` in service.go: absent is required, then the value must
/// be finite and fit the Float32 column.
fn measurement_value(name: &'static str, value: Option<f64>) -> Result<f32, DecodeError> {
    let Some(v) = value else {
        return Err(DecodeError::Required(name));
    };
    if v.is_nan() || v.is_infinite() || v.abs() > f32::MAX as f64 {
        return Err(DecodeError::NotFinite(name));
    }
    Ok(v as f32)
}

/// `occupancyCount` in pax.go: absent is required, then finite, whole, and
/// within the UInt32 column.
fn occupancy_count(value: Option<f64>) -> Result<u32, DecodeError> {
    let Some(v) = value else {
        return Err(DecodeError::Required("occupancy_count"));
    };
    if v.is_nan() || v.is_infinite() {
        return Err(DecodeError::OccupancyNotFinite);
    }
    if !(0.0..=4_294_967_295.0).contains(&v) {
        return Err(DecodeError::OccupancyRange);
    }
    // `v % 1.0` is exact in the range just validated (well below 2^53) and
    // needs no libm, so it stands in for Go's `*value != math.Trunc(*value)`.
    if v % 1.0 != 0.0 {
        return Err(DecodeError::OccupancyNotWhole);
    }
    Ok(v as u32)
}

/// Credentials as the service decodes them — borrowed from the body where it
/// has no escapes. An empty string means the key was absent or `null`; only
/// `sensor_id` is checked (the authorize stage), the rest are the decode-stage
/// zeros the service tolerates before authentication answers 401.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials<'a> {
    pub sensor_id: Cow<'a, str>,
    pub apikey: Cow<'a, str>,
    pub time: Cow<'a, str>,
}

/// The `/sensors/decibel` body as the service decodes it (`decibelPayload`).
#[derive(Clone, Debug, PartialEq)]
pub struct DecibelPayload<'a> {
    pub credentials: Credentials<'a>,
    /// Passed through unvalidated: the service's `rowID` regenerates a
    /// malformed id rather than rejecting it.
    pub measurement_id: Cow<'a, str>,
    pub dba: f32,
    pub dbc: f32,
}

/// The `/sensors/pax` body as the service decodes it (`paxPayload`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaxPayload<'a> {
    pub credentials: Credentials<'a>,
    pub measurement_id: Cow<'a, str>,
    pub occupancy_count: u32,
}

/// The `/sensors/weather` body as the service decodes it (`weatherPayload`).
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherPayload<'a> {
    pub credentials: Credentials<'a>,
    pub measurement_id: Cow<'a, str>,
    pub temperature: f32,
    pub humidity: f32,
    pub precipitation: f32,
    pub rain_probability: f32,
    pub wind_speed: f32,
    pub pressure: f32,
}

/// The `/sensors/status` body as the service decodes it (`statusPayload`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusPayload<'a> {
    pub credentials: Credentials<'a>,
    pub status_id: Cow<'a, str>,
    pub subsystem: Cow<'a, str>,
    pub result: StatusResult,
    pub message: Cow<'a, str>,
}

/// Decodes a `/sensors/decibel` body exactly as the service does: JSON
/// syntax, then `sensor_id` as UUID (the authorize stage), then `dba`/`dbc`
/// in `handleDecibel`'s order.
pub fn decode_decibel(input: &str) -> Result<DecibelPayload<'_>, DecodeError> {
    let mut sensor_id = Cow::Borrowed("");
    let mut apikey = Cow::Borrowed("");
    let mut time = Cow::Borrowed("");
    let mut measurement_id = Cow::Borrowed("");
    let mut dba = None;
    let mut dbc = None;
    let mut scan = Scanner::new(input);
    scan.fields(|key, raw| {
        match key {
            "sensor_id" => sensor_id = string_field("sensor_id", raw)?,
            "apikey" => apikey = string_field("apikey", raw)?,
            "time" => time = string_field("time", raw)?,
            "measurement_id" => measurement_id = string_field("measurement_id", raw)?,
            "dba" => dba = number_field("dba", raw)?,
            "dbc" => dbc = number_field("dbc", raw)?,
            _ => {}
        }
        Ok(())
    })?;
    scan.at_end()?;

    if !is_uuid(&sensor_id) {
        return Err(DecodeError::SensorIdNotUuid);
    }
    let dba = measurement_value("dba", dba)?;
    let dbc = measurement_value("dbc", dbc)?;
    Ok(DecibelPayload {
        credentials: Credentials {
            sensor_id,
            apikey,
            time,
        },
        measurement_id,
        dba,
        dbc,
    })
}

/// Decodes a `/sensors/pax` body exactly as the service does: JSON syntax,
/// then `sensor_id` as UUID, then `occupancyCount`'s checks.
pub fn decode_pax(input: &str) -> Result<PaxPayload<'_>, DecodeError> {
    let mut sensor_id = Cow::Borrowed("");
    let mut apikey = Cow::Borrowed("");
    let mut time = Cow::Borrowed("");
    let mut measurement_id = Cow::Borrowed("");
    let mut count = None;
    let mut scan = Scanner::new(input);
    scan.fields(|key, raw| {
        match key {
            "sensor_id" => sensor_id = string_field("sensor_id", raw)?,
            "apikey" => apikey = string_field("apikey", raw)?,
            "time" => time = string_field("time", raw)?,
            "measurement_id" => measurement_id = string_field("measurement_id", raw)?,
            "occupancy_count" => count = number_field("occupancy_count", raw)?,
            _ => {}
        }
        Ok(())
    })?;
    scan.at_end()?;

    if !is_uuid(&sensor_id) {
        return Err(DecodeError::SensorIdNotUuid);
    }
    let occupancy_count = occupancy_count(count)?;
    Ok(PaxPayload {
        credentials: Credentials {
            sensor_id,
            apikey,
            time,
        },
        measurement_id,
        occupancy_count,
    })
}

/// Decodes a `/sensors/weather` body exactly as the service does: JSON
/// syntax, then `sensor_id` as UUID, then the six fields in `handleWeather`'s
/// fixed order (so a body missing several reports the same one every time).
pub fn decode_weather(input: &str) -> Result<WeatherPayload<'_>, DecodeError> {
    let mut sensor_id = Cow::Borrowed("");
    let mut apikey = Cow::Borrowed("");
    let mut time = Cow::Borrowed("");
    let mut measurement_id = Cow::Borrowed("");
    let mut temperature = None;
    let mut humidity = None;
    let mut precipitation = None;
    let mut rain_probability = None;
    let mut wind_speed = None;
    let mut pressure = None;
    let mut scan = Scanner::new(input);
    scan.fields(|key, raw| {
        match key {
            "sensor_id" => sensor_id = string_field("sensor_id", raw)?,
            "apikey" => apikey = string_field("apikey", raw)?,
            "time" => time = string_field("time", raw)?,
            "measurement_id" => measurement_id = string_field("measurement_id", raw)?,
            "temperature" => temperature = number_field("temperature", raw)?,
            "humidity" => humidity = number_field("humidity", raw)?,
            "precipitation" => precipitation = number_field("precipitation", raw)?,
            "rain_probability" => {
                rain_probability = number_field("rain_probability", raw)?
            }
            "wind_speed" => wind_speed = number_field("wind_speed", raw)?,
            "pressure" => pressure = number_field("pressure", raw)?,
            _ => {}
        }
        Ok(())
    })?;
    scan.at_end()?;

    if !is_uuid(&sensor_id) {
        return Err(DecodeError::SensorIdNotUuid);
    }
    let temperature = measurement_value("temperature", temperature)?;
    let humidity = measurement_value("humidity", humidity)?;
    let precipitation = measurement_value("precipitation", precipitation)?;
    let rain_probability = measurement_value("rain_probability", rain_probability)?;
    let wind_speed = measurement_value("wind_speed", wind_speed)?;
    let pressure = measurement_value("pressure", pressure)?;
    Ok(WeatherPayload {
        credentials: Credentials {
            sensor_id,
            apikey,
            time,
        },
        measurement_id,
        temperature,
        humidity,
        precipitation,
        rain_probability,
        wind_speed,
        pressure,
    })
}

/// Decodes a `/sensors/status` body exactly as the service does: JSON syntax,
/// then `sensor_id` as UUID, then `subsystem` and `result` in `handleStatus`'s
/// order. `subsystem` is unvalidated beyond emptiness, mirroring the service.
pub fn decode_status(input: &str) -> Result<StatusPayload<'_>, DecodeError> {
    let mut sensor_id = Cow::Borrowed("");
    let mut apikey = Cow::Borrowed("");
    let mut time = Cow::Borrowed("");
    let mut status_id = Cow::Borrowed("");
    let mut subsystem = Cow::Borrowed("");
    let mut result = Cow::Borrowed("");
    let mut message = Cow::Borrowed("");
    let mut scan = Scanner::new(input);
    scan.fields(|key, raw| {
        match key {
            "sensor_id" => sensor_id = string_field("sensor_id", raw)?,
            "apikey" => apikey = string_field("apikey", raw)?,
            "time" => time = string_field("time", raw)?,
            "status_id" => status_id = string_field("status_id", raw)?,
            "subsystem" => subsystem = string_field("subsystem", raw)?,
            "result" => result = string_field("result", raw)?,
            "message" => message = string_field("message", raw)?,
            _ => {}
        }
        Ok(())
    })?;
    scan.at_end()?;

    if !is_uuid(&sensor_id) {
        return Err(DecodeError::SensorIdNotUuid);
    }
    if subsystem.is_empty() {
        return Err(DecodeError::SubsystemEmpty);
    }
    let Some(result) = StatusResult::parse(&result) else {
        return Err(DecodeError::ResultInvalid);
    };
    Ok(StatusPayload {
        credentials: Credentials {
            sensor_id,
            apikey,
            time,
        },
        status_id,
        subsystem,
        result,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::{Credentials as ModelCredentials, WeatherValues};

    const SID: &str = "0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10";
    const MID: &str = "0199d0e9-8b7c-7d4e-9f5a-2b3c4d5e6f70";
    const KEY: &str = "ck_live_01JNE8X4Q2R7T9WY3Z6A5B8CD";
    const T: &str = "2026-10-30T21:15:42.187Z";

    fn creds() -> ModelCredentials<'static> {
        ModelCredentials::new(SID, KEY, T).expect("test credentials are valid")
    }

    fn encode_to(bytes: &[u8]) -> &str {
        core::str::from_utf8(bytes).expect("encoder emits UTF-8")
    }

    #[test]
    fn decibel_round_trips_through_the_codec() {
        let m = DecibelMeasurement::new(creds(), MID, 72.5, 68.25).expect("valid");
        let mut buf = [0u8; 512];
        let n = encode_decibel(&m, &mut buf).expect("fits");
        let body = encode_to(&buf[..n]);
        let decoded = decode_decibel(body).expect("decodes");
        assert_eq!(decoded.credentials.sensor_id.as_ref(), SID);
        assert_eq!(decoded.credentials.apikey.as_ref(), KEY);
        assert_eq!(decoded.credentials.time.as_ref(), T);
        assert_eq!(decoded.measurement_id.as_ref(), MID);
        assert_eq!(decoded.dba, 72.5);
        assert_eq!(decoded.dbc, 68.25);
    }

    #[test]
    fn pax_round_trips_across_the_u32_range() {
        for count in [0, 1, 12, u32::MAX] {
            let m = PaxMeasurement::new(creds(), MID, count).expect("valid");
            let mut buf = [0u8; 512];
            let n = encode_pax(&m, &mut buf).expect("fits");
            let decoded = decode_pax(encode_to(&buf[..n])).expect("decodes");
            assert_eq!(decoded.occupancy_count, count);
            assert_eq!(decoded.measurement_id.as_ref(), MID);
        }
    }

    #[test]
    fn weather_round_trips_all_six_columns() {
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
        let decoded = decode_weather(encode_to(&buf[..n])).expect("decodes");
        assert_eq!(decoded.temperature, -4.5);
        assert_eq!(decoded.humidity, 55.25);
        assert_eq!(decoded.precipitation, 0.25);
        assert_eq!(decoded.rain_probability, 40.0);
        assert_eq!(decoded.wind_speed, 12.5);
        assert_eq!(decoded.pressure, 1013.25);
    }

    #[test]
    fn status_round_trips_with_escapes() {
        let m = StatusMeasurement::new(
            creds(),
            MID,
            "dsp",
            StatusResult::Degraded,
            "frames \"dropped\" a\\b\nnext\tline",
        )
        .expect("valid");
        let mut buf = [0u8; 512];
        let n = encode_status(&m, &mut buf).expect("fits");
        let decoded = decode_status(encode_to(&buf[..n])).expect("decodes");
        assert_eq!(decoded.subsystem.as_ref(), "dsp");
        assert_eq!(decoded.result, StatusResult::Degraded);
        assert_eq!(
            decoded.message.as_ref(),
            "frames \"dropped\" a\\b\nnext\tline"
        );
    }

    #[test]
    fn encoder_matches_the_go_field_order() {
        let m = DecibelMeasurement::new(creds(), MID, 72.5, 68.25).expect("valid");
        let mut buf = [0u8; 512];
        let n = encode_decibel(&m, &mut buf).expect("fits");
        let body = encode_to(&buf[..n]);
        assert!(body.starts_with("{\"sensor_id\":\""));
        assert!(body.contains("\",\"apikey\":\""));
        assert!(body.ends_with(",\"dba\":72.5,\"dbc\":68.25}"));
    }

    #[test]
    fn numbers_survive_f32_boundaries() {
        for value in [f32::MAX, -f32::MAX, -0.0, -12.75, 0.5] {
            let m = DecibelMeasurement::new(creds(), "", value, 0.0).expect("valid");
            let mut buf = [0u8; 512];
            let n = encode_decibel(&m, &mut buf).expect("fits");
            let decoded = decode_decibel(encode_to(&buf[..n])).expect("decodes");
            assert_eq!(decoded.dba, value);
        }
    }

    #[test]
    fn encoder_reports_overflow_instead_of_truncating() {
        let m = DecibelMeasurement::new(creds(), MID, 72.5, 68.25).expect("valid");
        let mut buf = [0u8; 32];
        assert_eq!(encode_decibel(&m, &mut buf), Err(EncodeError::Overflow));
        let mut exact = [0u8; 1];
        assert_eq!(encode_decibel(&m, &mut exact), Err(EncodeError::Overflow));
    }

    #[test]
    fn decoder_tolerates_unknown_and_duplicate_keys() {
        let body = r#"{"venue_id":"x","sensor_id":"0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10","apikey":"k","time":"2026-10-30T21:15:42Z","extra":{"deep":[1,2,{"a":true}],"n":null},"measurement_id":null,"dba":72.5,"dbc":68.25,"future":[true,false,null]}"#;
        let decoded = decode_decibel(body).expect("decodes");
        assert_eq!(decoded.credentials.apikey.as_ref(), "k");
        assert_eq!(decoded.measurement_id.as_ref(), "");
        assert_eq!(decoded.dba, 72.5);

        let dupes = r#"{"sensor_id":"0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10","apikey":"k","time":"t","dba":72.5,"dba":73.25,"dbc":0}"#;
        let decoded = decode_decibel(dupes).expect("decodes");
        assert_eq!(decoded.dba, 73.25);
    }

    #[test]
    fn decoder_accepts_whitespace_and_escapes() {
        let body = "{\n  \"sensor_id\" : \"0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10\" ,\n  \"apikey\":\"k\", \"time\":\"2026-10-30T21:15:42Z\",\n  \"measurement_id\":\"\", \"dba\":72.5, \"dbc\":68.25\n}";
        let decoded = decode_decibel(body).expect("decodes");
        assert_eq!(decoded.dba, 72.5);

        let escaped = r#"{"sensor_id":"0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10","apikey":"k/\u0041","time":"t","subsystem":"s","result":"success","message":"😀"}"#;
        let decoded = decode_status(escaped).expect("decodes");
        assert_eq!(decoded.credentials.apikey.as_ref(), "k/A");
        assert_eq!(decoded.message.as_ref(), "\u{1f600}");

        let paired = r#"{"sensor_id":"0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10","apikey":"k","time":"t","subsystem":"s","result":"success","message":"\ud83dX"}"#;
        let decoded = decode_status(paired).expect("decodes");
        assert_eq!(decoded.message.as_ref(), "\u{fffd}X");
    }

    #[test]
    fn decoder_rejects_what_the_service_rejects() {
        let head = r#"{"sensor_id":"0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10","apikey":"k","time":"t""#;
        assert_eq!(decode_decibel(""), Err(DecodeError::Syntax));
        assert_eq!(decode_decibel("[1]"), Err(DecodeError::Syntax));
        assert_eq!(decode_decibel("{"), Err(DecodeError::Syntax));
        assert_eq!(
            decode_decibel(r#"{"sensor_id":1}"#),
            Err(DecodeError::WrongType("sensor_id"))
        );
        assert_eq!(decode_decibel(r#"{bad}"#), Err(DecodeError::Syntax));
        assert_eq!(
            decode_decibel(&format!("{head},\"dba\":1,\"dbc\":2}}x")),
            Err(DecodeError::Syntax)
        );
        // authorize stage: sensor_id must be a UUID.
        assert_eq!(
            decode_decibel(
                r#"{"sensor_id":"nope","apikey":"k","time":"t","dba":1,"dbc":2}"#
            ),
            Err(DecodeError::SensorIdNotUuid)
        );
        // handleDecibel: required, then typed, then finite.
        assert_eq!(
            decode_decibel(&format!("{head},\"dbc\":2}}")),
            Err(DecodeError::Required("dba"))
        );
        assert_eq!(
            decode_decibel(&format!("{head},\"dba\":\"72\",\"dbc\":2}}")),
            Err(DecodeError::WrongType("dba"))
        );
        assert_eq!(
            decode_decibel(&format!("{head},\"dba\":null,\"dbc\":2}}")),
            Err(DecodeError::Required("dba"))
        );
        assert_eq!(
            decode_decibel(&format!("{head},\"dba\":1e999,\"dbc\":2}}")),
            Err(DecodeError::NotFinite("dba"))
        );
        assert_eq!(
            decode_decibel(&format!("{head},\"dba\":1,\"dbc\":2}},\"dbc\":3}}")),
            Err(DecodeError::Syntax)
        );
        // occupancyCount's ladder.
        let pax = |v: &str| {
            format!(
                r#"{{"sensor_id":"0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10","apikey":"k","time":"t","occupancy_count":{v}}}"#
            )
        };
        assert_eq!(decode_pax(&pax("7.5")), Err(DecodeError::OccupancyNotWhole));
        assert_eq!(decode_pax(&pax("-1")), Err(DecodeError::OccupancyRange));
        assert_eq!(
            decode_pax(&pax("4294967296")),
            Err(DecodeError::OccupancyRange)
        );
        assert_eq!(
            decode_pax(&pax("\"7\"")),
            Err(DecodeError::WrongType("occupancy_count"))
        );
        // handleStatus's checks, in order.
        let status_body = |fields: &str| {
            format!(
                r#"{{"sensor_id":"0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10","apikey":"k","time":"t",{fields}}}"#
            )
        };
        assert_eq!(
            decode_status(&status_body(r#""subsystem":"","result":"success""#)),
            Err(DecodeError::SubsystemEmpty)
        );
        assert_eq!(
            decode_status(&status_body(r#""subsystem":"dsp","result":"ok""#)),
            Err(DecodeError::ResultInvalid)
        );
        assert_eq!(
            decode_status(&status_body(r#""subsystem":"dsp""#)),
            Err(DecodeError::ResultInvalid)
        );
        // handleWeather: the fixed check order reports the first missing field.
        let weather_body = |fields: &str| {
            format!(
                r#"{{"sensor_id":"0199d0e9-8b7a-7cc2-9fd3-8f2b6f1e4a10","apikey":"k","time":"t",{fields}}}"#
            )
        };
        assert_eq!(
            decode_weather(&weather_body(
                r#""humidity":1,"precipitation":1,"rain_probability":1,"wind_speed":1,"pressure":1"#
            )),
            Err(DecodeError::Required("temperature"))
        );
        assert_eq!(
            decode_weather(&weather_body(
                r#""temperature":1,"humidity":1,"precipitation":1,"rain_probability":1,"pressure":1"#
            )),
            Err(DecodeError::Required("wind_speed"))
        );
    }
}
