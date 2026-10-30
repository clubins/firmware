//! A fixed-capacity TOML subset — exactly enough for `config/default.toml`
//! and for NVS override values.
//!
//! The crate is zero-dependency and allocation-free (see `lib.rs`), so the
//! config layer carries its own scanner instead of pulling in a TOML crate.
//! Supported syntax: comments, `[table]` and dotted `[a.b]` headers, bare
//! keys, strings with a small escape set, integers (decimal and `0x` hex,
//! `_` separators allowed), floats with a fraction part, and booleans.
//! Anything else is a [`ParseError`].
//!
//! Unknown keys and unknown tables are deliberately **not** errors — they
//! land in [`Table::warnings`] and the typed config keeps its default
//! (spec §Configuration: a config edit re-tunes tunables, it cannot invent
//! new schema).
//!
//! Like [`crate::ring::Spsc`] and the codec in `transports`, everything
//! here takes caller-provided storage: [`parse`] fills a reusable
//! [`Table`] scratch (on the host stack in tests, pinned in a `StaticCell`
//! on device) — no heap, ever.

use core::fmt;

/// Longest dotted key path we can record (e.g.
/// `sampling.decibel_window_ms`).
pub const MAX_KEY_LEN: usize = 48;

/// Longest string value we can hold (the HTTP endpoint is the longest
/// value in `default.toml` with a wide margin).
pub const MAX_VALUE_LEN: usize = 128;

/// Entries per parse scratch. `default.toml` uses 18; the rest is
/// headroom for a deployment file with unknown keys (each still recorded,
/// each still warned about).
pub const MAX_ENTRIES: usize = 64;

/// Warnings kept per parse. Past this the first warnings stay and
/// [`Warnings::truncated`] flips — never silently dropped.
pub const MAX_WARNINGS: usize = 16;

// ---------------------------------------------------------------------------
// FixedStr — an inline, fixed-capacity string
// ---------------------------------------------------------------------------

/// An inline string with a compile-time capacity. The no-alloc stand-in
/// for `String` — config identity and endpoint values are short and
/// bounded, so the capacity is part of the type.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FixedStr<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> FixedStr<N> {
    /// An empty string.
    #[inline]
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    /// Copies `s` in, or `None` when it exceeds the capacity. Const:
    /// the built-in layer is a `const`.
    pub const fn from_str(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() > N {
            return None;
        }
        let mut out = Self {
            bytes: [0; N],
            len: bytes.len(),
        };
        let mut i = 0;
        while i < bytes.len() {
            out.bytes[i] = bytes[i];
            i += 1;
        }
        Some(out)
    }

    /// Copies `s` over the current content. Returns `false` (content
    /// unchanged) when `s` exceeds the capacity.
    pub fn set(&mut self, s: &str) -> bool {
        if s.len() > N {
            return false;
        }
        self.bytes = [0; N];
        self.bytes[..s.len()].copy_from_slice(s.as_bytes());
        self.len = s.len();
        true
    }

    /// Appends `s`, or returns `false` (content unchanged) when that
    /// would exceed the capacity.
    pub fn push_str(&mut self, s: &str) -> bool {
        if self.len + s.len() > N {
            return false;
        }
        self.bytes[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
        true
    }

    /// Empties the string, keeping the storage.
    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// The content.
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY of the invariant, not of memory: bytes below `len` were
        // copied from a `&str` and are valid UTF-8; the rest is zeros.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    /// Content length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> Default for FixedStr<N> {
    fn default() -> Self {
        Self::new()
    }
}

// Manual Debug: show the content, not the zero padding.
impl<const N: usize> fmt::Debug for FixedStr<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// A parsed scalar value — one integer, float, bool, or string.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f32),
    Str(FixedStr<MAX_VALUE_LEN>),
}

impl Value {
    /// The integer content, or `None` for any other kind.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The float content (finite), or `None` for any other kind.
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            // NaN/inf are valid TOML floats but meaningless as tunables.
            Value::Float(f) if f.is_finite() => Some(*f),
            _ => None,
        }
    }

    /// The bool content, or `None` for any other kind.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The string content, or `None` for any other kind.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// One `key = value` binding, keyed by its full dotted path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    /// Full dotted path as written (header + key joined with dots).
    pub key: FixedStr<MAX_KEY_LEN>,
    pub value: Value,
}

// ---------------------------------------------------------------------------
// Errors and warnings
// ---------------------------------------------------------------------------

/// Why a line could not be parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// The line is not a comment, blank line, header, or `key = value`.
    Syntax,
    /// A `[header]` is malformed (unterminated, empty segment, bad char).
    BadHeader,
    /// A numeric literal is malformed or overflows.
    BadNumber,
    /// A string literal is malformed (unterminated quote, bad escape).
    BadString,
    /// Valid TOML this scanner deliberately does not support (arrays,
    /// multiline strings, exponent floats, quoted keys).
    Unsupported,
    /// The same dotted path appears twice.
    DuplicateKey,
    /// A string value exceeds [`MAX_VALUE_LEN`] (or a key
    /// [`MAX_KEY_LEN`]).
    ValueTooLong,
}

/// A parse failure, anchored to its line (1-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line number.
    pub line: usize,
    pub kind: ParseErrorKind,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {:?}", self.line, self.kind)
    }
}

impl core::error::Error for ParseError {}

/// What a tolerated-but-noteworthy input was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarningKind {
    /// The key is not in the config schema — kept on its layer's default.
    UnknownKey,
    /// The table is not in the config schema — its keys warn individually.
    UnknownTable,
    /// An NVS override value for a known key did not parse or validate.
    BadOverride,
    /// Reading an override from the source failed (device NVS I/O).
    OverrideReadFailed,
}

/// One tolerated-but-noteworthy input, with the key or table it concerns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Warning {
    pub kind: WarningKind,
    /// The dotted key or table path.
    pub path: FixedStr<MAX_KEY_LEN>,
}

/// A bounded list of [`Warning`]s. Pushing past [`MAX_WARNINGS`] keeps the
/// first ones and flips [`Warnings::truncated`] — the list never lies
/// about being incomplete.
#[derive(Clone, Copy)]
pub struct Warnings {
    items: [Option<Warning>; MAX_WARNINGS],
    len: usize,
    truncated: bool,
}

impl Warnings {
    /// An empty list.
    #[inline]
    pub const fn new() -> Self {
        Self {
            items: [const { None }; MAX_WARNINGS],
            len: 0,
            truncated: false,
        }
    }

    /// Appends a warning, or marks the list truncated when full.
    pub fn push(&mut self, w: Warning) {
        if self.len == MAX_WARNINGS {
            self.truncated = true;
            return;
        }
        self.items[self.len] = Some(w);
        self.len += 1;
    }

    /// Number of warnings kept.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when nothing was warned about.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// True when warnings were dropped for room.
    #[inline]
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// True when the list contains `kind` for `path` (for tests and for
    /// callers surfacing a specific issue).
    pub fn has(&self, kind: WarningKind, path: &str) -> bool {
        self.items[..self.len]
            .iter()
            .any(|w| w.is_some_and(|w| w.kind == kind && w.path.as_str() == path))
    }

    /// The warnings, in order.
    pub fn iter(&self) -> impl Iterator<Item = &Warning> {
        self.items[..self.len].iter().flatten()
    }

    /// Copies every warning of `other` in.
    pub fn merge(&mut self, other: &Warnings) {
        for w in other.iter() {
            self.push(*w);
        }
    }
}

impl Default for Warnings {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Warnings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.items[..self.len].iter().flatten())
            .finish()?;
        if self.truncated {
            write!(f, " (truncated)")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Table — the parse scratch
// ---------------------------------------------------------------------------

/// The parse result: every `key = value` in the file, plus the warnings
/// for what was tolerated. Reused as scratch across parses — see
/// [`parse`].
#[derive(Debug)]
pub struct Table {
    entries: [Option<Entry>; MAX_ENTRIES],
    len: usize,
    warnings: Warnings,
}

impl Table {
    /// An empty table (const: usable in a `StaticCell`).
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_ENTRIES],
            len: 0,
            warnings: Warnings::new(),
        }
    }

    /// Discards all content and warnings, keeping the storage.
    pub fn clear(&mut self) {
        self.entries = [const { None }; MAX_ENTRIES];
        self.len = 0;
        self.warnings = Warnings::new();
    }

    /// The parsed entries, in file order.
    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.entries[..self.len].iter().flatten()
    }

    /// The value for a full dotted path, or `None`.
    pub fn get(&self, path: &str) -> Option<&Value> {
        self.entries[..self.len].iter().flatten().find_map(|e| {
            if e.key.as_str() == path {
                Some(&e.value)
            } else {
                None
            }
        })
    }

    /// True when the parse produced a `kind` warning for `path`
    /// (delegates to the table's warning list).
    pub fn has(&self, kind: WarningKind, path: &str) -> bool {
        self.warnings.has(kind, path)
    }

    /// The tolerated-input warnings from the last parse.
    pub fn warnings(&self) -> &Warnings {
        &self.warnings
    }

    fn record(&mut self, key: &str, value: Value, line: usize) -> Result<(), ParseError> {
        if self.len == MAX_ENTRIES {
            return Err(ParseError {
                line,
                kind: ParseErrorKind::Unsupported,
            });
        }
        let key = FixedStr::from_str(key).ok_or(ParseError {
            line,
            kind: ParseErrorKind::ValueTooLong,
        })?;
        if self.entries[..self.len]
            .iter()
            .flatten()
            .any(|e| e.key == key)
        {
            return Err(ParseError {
                line,
                kind: ParseErrorKind::DuplicateKey,
            });
        }
        self.entries[self.len] = Some(Entry { key, value });
        self.len += 1;
        Ok(())
    }

    fn warn(&mut self, kind: WarningKind, path: &str) {
        if let Some(path) = FixedStr::from_str(path) {
            self.warnings.push(Warning { kind, path });
        }
    }
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// Parses `text` into `out` (cleared first). `known_tables` are the table
/// paths the schema owns — headers outside the list produce
/// [`WarningKind::UnknownTable`] warnings instead of errors. On `Err`,
/// `out` holds the entries recorded before the failing line.
pub fn parse<'a>(
    text: &str,
    known_tables: &[&str],
    out: &'a mut Table,
) -> Result<&'a Table, ParseError> {
    out.clear();
    let mut header = "";
    let mut path = FixedStr::<MAX_KEY_LEN>::new();
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = strip_comment(raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(inner) = header_of(line) {
            let path = parse_header(inner).ok_or(ParseError {
                line: line_no,
                kind: ParseErrorKind::BadHeader,
            })?;
            if !known_tables.contains(&path) {
                out.warn(WarningKind::UnknownTable, path);
            }
            header = path;
            continue;
        }
        if line.starts_with('[') {
            // A line that opens a header but never closes it is a header
            // defect, not generic syntax.
            return Err(ParseError {
                line: line_no,
                kind: ParseErrorKind::BadHeader,
            });
        }

        let (key, rest) = split_key(line).ok_or(ParseError {
            line: line_no,
            kind: ParseErrorKind::Syntax,
        })?;
        let value = parse_value(rest).map_err(|kind| ParseError {
            line: line_no,
            kind,
        })?;
        if !join_path(header, key, &mut path) {
            return Err(ParseError {
                line: line_no,
                kind: ParseErrorKind::ValueTooLong,
            });
        }
        out.record(path.as_str(), value, line_no)?;
    }
    Ok(out)
}

/// Parses a single scalar value — the shared entry point for NVS override
/// values, which arrive as raw text rather than TOML lines.
pub fn parse_value(s: &str) -> Result<Value, ParseErrorKind> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseErrorKind::Syntax);
    }
    if s == "true" {
        return Ok(Value::Bool(true));
    }
    if s == "false" {
        return Ok(Value::Bool(false));
    }
    if let Some(body) = s.strip_prefix('"') {
        let inner = scan_string(body)?;
        // Anything between the closing quote and end-of-token is trailing
        // garbage (a comment should already have been stripped).
        if !body[inner.len() + 1..].trim().is_empty() {
            return Err(ParseErrorKind::Syntax);
        }
        return decode_string(inner);
    }
    if let Some(digits) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let mut num = [0u8; MAX_VALUE_LEN];
        let hex = strip_underscores(digits, &mut num);
        return i64::from_str_radix(hex, 16)
            .map(Value::Int)
            .map_err(|_| ParseErrorKind::BadNumber);
    }
    let mut num = [0u8; MAX_VALUE_LEN];
    if strip_underscores(s, &mut num).contains('.') {
        // Fraction part required; other float spellings (exponents) fail
        // the decimal parse below.
        let f = strip_underscores(s, &mut num)
            .parse::<f32>()
            .map_err(|_| ParseErrorKind::BadNumber)?;
        if !f.is_finite() {
            // NaN/inf are valid TOML floats but meaningless as tunables.
            return Err(ParseErrorKind::BadNumber);
        }
        return Ok(Value::Float(f));
    }
    strip_underscores(s, &mut num)
        .parse::<i64>()
        .map(Value::Int)
        .map_err(|_| ParseErrorKind::BadNumber)
}

/// Finds the closing quote of a string body (escape-aware) and returns
/// the content between the quotes.
fn scan_string(after_open: &str) -> Result<&str, ParseErrorKind> {
    let mut escaped = false;
    for (i, c) in after_open.char_indices() {
        if escaped {
            escaped = false;
        } else {
            match c {
                '\\' => escaped = true,
                '"' => return Ok(&after_open[..i]),
                _ => {}
            }
        }
    }
    Err(ParseErrorKind::BadString)
}

/// Cuts an unquoted `#` comment off a line, respecting string literals.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (i, b) in line.bytes().enumerate() {
        match (in_string, escaped, b) {
            (true, true, _) => escaped = false,
            (true, false, b'\\') => escaped = true,
            (true, false, b'"') => in_string = false,
            (false, _, b'"') => in_string = true,
            (false, _, b'#') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// `[table]` → `table`, with the trailing `]` verified.
fn header_of(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner.trim())
}

/// Validates a dotted header path (`sensors.bme280`).
fn parse_header(inner: &str) -> Option<&str> {
    for seg in inner.split('.') {
        if seg.is_empty() || !seg.bytes().all(is_bare_key_byte) {
            return None;
        }
    }
    Some(inner)
}

/// `key = rest` on a pair line (bare key only; dotted pair keys are not
/// needed — headers carry the nesting).
fn split_key(line: &str) -> Option<(&str, &str)> {
    let eq = line.find('=')?;
    let key = line[..eq].trim();
    if key.is_empty() || !key.bytes().all(is_bare_key_byte) {
        return None;
    }
    Some((key, &line[eq + 1..]))
}

fn is_bare_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Joins the current header path with a pair key (`transport` + `port` →
/// `transport.port`) into `out`; `false` when the joined path overflows
/// [`MAX_KEY_LEN`].
fn join_path(header: &str, key: &str, out: &mut FixedStr<MAX_KEY_LEN>) -> bool {
    out.clear();
    if !header.is_empty() && !(out.push_str(header) && out.push_str(".")) {
        return false;
    }
    out.push_str(key)
}

/// Decodes the body of a quoted string (escape set: `\"`, `\\`, `\n`,
/// `\r`, `\t`).
fn decode_string(body: &str) -> Result<Value, ParseErrorKind> {
    let mut out = FixedStr::<MAX_VALUE_LEN>::new();
    let mut chars = body.char_indices();
    while let Some((_, c)) = chars.next() {
        if c != '\\' {
            if !push_char(&mut out, c) {
                return Err(ParseErrorKind::ValueTooLong);
            }
            continue;
        }
        let (_, esc) = chars.next().ok_or(ParseErrorKind::BadString)?;
        let unescaped = match esc {
            '"' => '"',
            '\\' => '\\',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            _ => return Err(ParseErrorKind::BadString),
        };
        if !push_char(&mut out, unescaped) {
            return Err(ParseErrorKind::ValueTooLong);
        }
    }
    Ok(Value::Str(out))
}

fn push_char(out: &mut FixedStr<MAX_VALUE_LEN>, c: char) -> bool {
    // Encode one char into a small scratch and append its bytes.
    let mut buf = [0u8; 4];
    out.push_str(c.encode_utf8(&mut buf))
}

/// Removes `_` digit separators from a numeric token into `out`,
/// returning the cleaned text. Only separator bytes are dropped, so a
/// token longer than `out` is returned unchanged (the numeric parse then
/// fails — same outcome as before the guard, no overflow). Without a
/// separator the input borrows back unchanged.
fn strip_underscores<'a>(s: &'a str, out: &'a mut [u8]) -> &'a str {
    if !s.contains('_') {
        return s;
    }
    if s.len() > out.len() {
        return s;
    }
    let mut n = 0;
    for b in s.as_bytes() {
        if *b != b'_' {
            out[n] = *b;
            n += 1;
        }
    }
    // Tokens reaching here are ASCII numerics, so the cleaned bytes are
    // valid UTF-8.
    core::str::from_utf8(&out[..n]).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLES: &[&str] = &["sensor", "transport", "transport.mqtt"];

    #[test]
    fn scalars_parse() {
        assert_eq!(parse_value("true"), Ok(Value::Bool(true)));
        assert_eq!(parse_value("60"), Ok(Value::Int(60)));
        assert_eq!(parse_value("-3"), Ok(Value::Int(-3)));
        assert_eq!(parse_value("0x76"), Ok(Value::Int(0x76)));
        assert_eq!(parse_value("1_000"), Ok(Value::Int(1000)));
        assert_eq!(parse_value("120.0"), Ok(Value::Float(120.0)));
        assert_eq!(parse_value("-1.5"), Ok(Value::Float(-1.5)));
        assert_eq!(
            parse_value("\"http\""),
            Ok(Value::Str(FixedStr::from_str("http").unwrap()))
        );
    }

    #[test]
    fn strings_decode_escapes() {
        assert_eq!(
            parse_value("\"a\\nb\""),
            Ok(Value::Str(FixedStr::from_str("a\nb").unwrap()))
        );
        assert_eq!(
            parse_value("\"a\\\\b\""),
            Ok(Value::Str(FixedStr::from_str("a\\b").unwrap()))
        );
        assert_eq!(parse_value("\"open"), Err(ParseErrorKind::BadString));
        assert_eq!(parse_value("\"bad\\xq\""), Err(ParseErrorKind::BadString));
        // An escaped quote does not end the string.
        assert_eq!(
            parse_value("\"a\\\"b\""),
            Ok(Value::Str(FixedStr::from_str("a\"b").unwrap()))
        );
    }

    #[test]
    fn bad_numbers_are_rejected() {
        assert_eq!(parse_value("1.2.3"), Err(ParseErrorKind::BadNumber));
        assert_eq!(parse_value("12ab"), Err(ParseErrorKind::BadNumber));
        assert_eq!(parse_value(""), Err(ParseErrorKind::Syntax));
        // Integers must fit i64.
        assert_eq!(
            parse_value("99999999999999999999"),
            Err(ParseErrorKind::BadNumber)
        );
    }

    #[test]
    fn nan_and_inf_are_not_tunables() {
        assert_eq!(parse_value("nan"), Err(ParseErrorKind::BadNumber));
        assert_eq!(parse_value("inf"), Err(ParseErrorKind::BadNumber));
    }

    #[test]
    fn file_parses_with_headers_and_inline_comments() {
        let mut t = Table::new();
        let text = "# leading comment\n\
                    [sensor]\n\
                    id = \"00000000-0000-0000-0000-000000000000\"  # real UUID in NVS\n\
                    \n\
                    [transport.mqtt]\n\
                    port = 8883  # TLS listener\n";
        parse(text, TABLES, &mut t).expect("parse");
        assert_eq!(t.iter().count(), 2);
        assert_eq!(
            t.get("sensor.id"),
            Some(&Value::Str(
                FixedStr::from_str("00000000-0000-0000-0000-000000000000").unwrap()
            ))
        );
        assert_eq!(t.get("transport.mqtt.port"), Some(&Value::Int(8883)));
        assert!(t.warnings().is_empty());
    }

    #[test]
    fn unknown_keys_and_tables_warn_instead_of_failing() {
        let mut t = Table::new();
        let text = "[sampling]\n\
                    report_interval_s = 60\n\
                    future_tunable = 1\n\
                    [vendor.custom]\n\
                    thing = 2\n";
        parse(text, TABLES, &mut t).expect("unknowns are tolerated");
        // Unknown TABLES warn at parse level (the parser knows tables, not
        // the schema's key paths); unknown KEYS surface through the config
        // layer's resolution — see tests/config_conformance.rs.
        assert!(t.has(WarningKind::UnknownTable, "vendor.custom"));
        // Both keys still parsed, known and unknown alike.
        assert_eq!(t.get("sampling.report_interval_s"), Some(&Value::Int(60)));
        assert_eq!(t.get("sampling.future_tunable"), Some(&Value::Int(1)));
    }

    #[test]
    fn duplicate_keys_are_errors() {
        let mut t = Table::new();
        let text = "[transport]\nport = 1\nport = 2\n";
        let err = parse(text, TABLES, &mut t).expect_err("duplicate");
        assert_eq!(err.kind, ParseErrorKind::DuplicateKey);
        assert_eq!(err.line, 3);
    }

    #[test]
    fn unsupported_shapes_are_errors() {
        let mut t = Table::new();
        assert_eq!(
            parse("[transport]\nports = [1, 2]\n", TABLES, &mut t)
                .unwrap_err()
                .kind,
            ParseErrorKind::BadNumber
        );
        assert_eq!(
            parse("dotted.key = 1\n", TABLES, &mut t).unwrap_err().kind,
            ParseErrorKind::Syntax
        );
        assert_eq!(
            parse("malformed\n", TABLES, &mut t).unwrap_err().kind,
            ParseErrorKind::Syntax
        );
        assert_eq!(
            parse("[unterminated\n", TABLES, &mut t).unwrap_err().kind,
            ParseErrorKind::BadHeader
        );
    }

    #[test]
    fn warning_list_reports_truncation() {
        let mut w = Warnings::new();
        for i in 0..MAX_WARNINGS {
            w.push(Warning {
                kind: WarningKind::UnknownKey,
                path: FixedStr::from_str("a").unwrap(),
            });
            assert!(!w.truncated());
            assert_eq!(w.len(), i + 1);
        }
        w.push(Warning {
            kind: WarningKind::UnknownKey,
            path: FixedStr::from_str("overflow").unwrap(),
        });
        assert!(w.truncated());
        assert_eq!(w.len(), MAX_WARNINGS);
        assert!(!w.has(WarningKind::UnknownKey, "overflow"));
    }

    #[test]
    fn fixed_str_round_trips() {
        let mut s = FixedStr::<8>::new();
        assert!(s.is_empty());
        assert!(s.set("abcdef"));
        assert_eq!(s.as_str(), "abcdef");
        assert!(!s.set("123456789")); // over capacity — content unchanged
        assert_eq!(s.as_str(), "abcdef");
        assert_eq!(FixedStr::<8>::from_str("123456789"), None);
    }

    #[test]
    fn comment_stripper_respects_strings() {
        assert_eq!(strip_comment("a = \"x # y\"  # tail"), "a = \"x # y\"  ");
        assert_eq!(strip_comment("# whole line"), "");
        assert_eq!(strip_comment("port = 8883"), "port = 8883");
    }
}
