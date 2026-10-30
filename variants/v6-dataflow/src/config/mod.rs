//! The runtime config schema and its three-layer precedence.
//!
//! Spec §Configuration: one canonical `config/default.toml` is committed
//! and compiled into the binary as the defaults ([`DEFAULT_TOML`]); at
//! boot, NVS overrides are applied on top. Precedence is one rule, tested
//! on the host:
//!
//! ```text
//! built-in constants  <  config/default.toml  <  NVS overrides
//! ```
//!
//! Secrets — Wi-Fi credentials, the ingest API key, the mTLS client
//! certificate and key — live only in NVS. They are not fields on any
//! config struct, [`KNOWN_PATHS`] does not contain their keys, and
//! [`Config::set_field`] carries a `debug_assert` tripwire — so neither a
//! file entry nor an override can route a secret into a `Config`.
//! Consumers (transports, Wi-Fi bring-up) read secrets from the
//! [`crate::nvs::OverrideSource`] directly, at their point of use.
//!
//! Wrong-typed values on *known* keys are hard errors — a committed file
//! is reviewed code and should fail fast (and the host tests parse
//! [`DEFAULT_TOML`] itself, so a bad committed value fails CI before any
//! device sees it). Bad values arriving through *NVS* are only warnings —
//! one unparseable per-device entry must not keep a deployed device off
//! the air; it stays on the layer below.
//!
//! Zero dependencies, zero heap: parsing fills a caller-provided
//! [`toml::Table`] scratch and rendering writes a caller-provided buffer,
//! like every other stage in this crate.

mod toml;

use core::fmt::Write as _;

pub use toml::{
    Entry, FixedStr, MAX_KEY_LEN, MAX_VALUE_LEN, ParseError, ParseErrorKind, Table,
    Value, Warning, WarningKind, Warnings, parse_value,
};

use crate::nvs::OverrideSource;

/// Longest transport endpoint value the schema holds.
const ENDPOINT_MAX: usize = MAX_VALUE_LEN;

/// Longest MQTT broker host name the schema holds.
const BROKER_MAX: usize = 64;

/// UUID text length (`8-4-4-4-12`).
const UUID_LEN: usize = 36;

/// Fast-window length the aggregate decibel window must fold (decibel.md).
const FAST_WINDOW_MS: u32 = 125;

/// The committed defaults, compiled into the binary (spec §Configuration:
/// "compiled into the binary as the defaults").
pub const DEFAULT_TOML: &str = include_str!("../../config/default.toml");

/// The table paths the schema owns. Anything else warns as unknown.
pub const KNOWN_TABLES: &[&str] = &[
    "sensor",
    "sampling",
    "sensors",
    "sensors.bme280",
    "sensors.sht4x",
    "sensors.scd4x",
    "sensors.bh1750",
    "sensors.pir",
    "sensors.pms5003",
    "transport",
    "transport.mqtt",
    "decibel",
];

/// Every tunable path the schema owns, in canonical order. Single source
/// of truth for the override layer ([`Config::apply_overrides`] walks it
/// in this order) and for the drift tests.
pub const KNOWN_PATHS: &[&str] = &[
    "sensor.id",
    "sampling.decibel_window_ms",
    "sampling.report_interval_s",
    "sampling.offline_buffer",
    "sensors.bme280.i2c_address",
    "sensors.sht4x.i2c_address",
    "sensors.scd4x.i2c_address",
    "sensors.bh1750.i2c_address",
    "sensors.pir.gpio_pin",
    "sensors.pms5003.uart_baud",
    "transport.active",
    "transport.http_endpoint",
    "transport.mqtt.broker",
    "transport.mqtt.port",
    "decibel.overload_threshold_db",
    "decibel.calibration_offset_db",
];

/// Why a config could not be resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A layer failed to parse; the line is 1-based within that layer's
    /// text.
    Parse { line: usize, kind: ParseErrorKind },
    /// A known key got a value of the wrong type, out of its documented
    /// range, or (for `sensor.id`) not shaped like a UUID.
    BadValue { path: &'static str },
    /// A string value exceeded its field's capacity.
    ValueTooLong { path: &'static str },
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConfigError::Parse { line, kind } => {
                write!(f, "config parse error: {kind:?} at line {line}")
            }
            ConfigError::BadValue { path } => write!(f, "bad value for {path}"),
            ConfigError::ValueTooLong { path } => write!(f, "value too long for {path}"),
        }
    }
}

impl core::error::Error for ConfigError {}

impl From<ParseError> for ConfigError {
    fn from(e: ParseError) -> Self {
        ConfigError::Parse {
            line: e.line,
            kind: e.kind,
        }
    }
}

/// The compiled-in transport selection (config: `transport.active`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportKind {
    Http,
    Mqtt,
}

impl TransportKind {
    fn from_name(s: &str) -> Option<Self> {
        match s {
            "http" => Some(TransportKind::Http),
            "mqtt" => Some(TransportKind::Mqtt),
            _ => None,
        }
    }

    /// The name the config file and the status page use.
    pub const fn as_str(&self) -> &'static str {
        match self {
            TransportKind::Http => "http",
            TransportKind::Mqtt => "mqtt",
        }
    }
}

// ---------------------------------------------------------------------------
// Schema structs — one per table in `config/default.toml`
// ---------------------------------------------------------------------------

/// `[sensor]` — device identity on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorConfig {
    /// The sensor UUID as text. The committed value is a placeholder —
    /// the real identity is an NVS override per device.
    pub id: FixedStr<UUID_LEN>,
}

/// `[sampling]` — measurement cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplingConfig {
    /// Aggregate window for decibel reporting; decibel.md's Fast windows
    /// (125 ms) fold inside it, so it must be a multiple of 125.
    pub decibel_window_ms: u32,
    /// Per-type reporting cadence.
    pub report_interval_s: u32,
    /// Measurements held while disconnected (spec: a bounded offline
    /// buffer, drained when connectivity returns).
    pub offline_buffer: u32,
}

/// An I2C device address (7-bit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I2cDevice {
    pub i2c_address: u8,
}

/// The PIR occupancy front-end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PirDevice {
    pub gpio_pin: u8,
}

/// The PMS5003 particulates sensor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UartDevice {
    pub uart_baud: u32,
}

/// `[sensors.*]` — per-driver hardware selections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorDevices {
    pub bme280: I2cDevice,
    pub sht4x: I2cDevice,
    pub scd4x: I2cDevice,
    pub bh1750: I2cDevice,
    pub pir: PirDevice,
    pub pms5003: UartDevice,
}

/// `[transport.mqtt]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MqttConfig {
    pub broker: FixedStr<BROKER_MAX>,
    pub port: u16,
}

/// `[transport]` — which compiled transport is active, and its addressing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportConfig {
    pub active: TransportKind,
    pub http_endpoint: FixedStr<ENDPOINT_MAX>,
    pub mqtt: MqttConfig,
}

/// `[decibel]` — the DSP tunables decibel.md marks configurable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecibelConfig {
    /// Above this A-weighted level a window flags an overload.
    pub overload_threshold_db: f32,
    /// Per-unit calibration offset — the committed value is neutral; the
    /// real offset is an NVS override from provisioning.
    pub calibration_offset_db: f32,
}

/// The resolved runtime configuration: every tunable the firmware reads,
/// after layering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub sensor: SensorConfig,
    pub sampling: SamplingConfig,
    pub sensors: SensorDevices,
    pub transport: TransportConfig,
    pub decibel: DecibelConfig,
}

// ---------------------------------------------------------------------------
// Resolution — built-in < file < NVS
// ---------------------------------------------------------------------------

impl Config {
    /// The built-in constants — the lowest layer, and the values
    /// `config/default.toml` documents (a drift test pins the two
    /// together). `expect`s fire at compile time if a literal outgrows
    /// its field.
    pub const BUILT_IN: Self = Self {
        sensor: SensorConfig {
            id: FixedStr::from_str("00000000-0000-0000-0000-000000000000")
                .expect("built-in UUID fits its field"),
        },
        sampling: SamplingConfig {
            decibel_window_ms: 1_000,
            report_interval_s: 60,
            offline_buffer: 64,
        },
        sensors: SensorDevices {
            bme280: I2cDevice { i2c_address: 0x76 },
            sht4x: I2cDevice { i2c_address: 0x44 },
            scd4x: I2cDevice { i2c_address: 0x62 },
            bh1750: I2cDevice { i2c_address: 0x23 },
            pir: PirDevice { gpio_pin: 4 },
            pms5003: UartDevice { uart_baud: 9_600 },
        },
        transport: TransportConfig {
            active: TransportKind::Http,
            http_endpoint: FixedStr::from_str("https://ingest.clubins.de/v1/sensors")
                .expect("built-in endpoint fits its field"),
            mqtt: MqttConfig {
                broker: FixedStr::from_str("mqtt.clubins.de")
                    .expect("built-in broker fits its field"),
                port: 8883,
            },
        },
        decibel: DecibelConfig {
            overload_threshold_db: 120.0,
            calibration_offset_db: 0.0,
        },
    };

    /// Resolves the runtime config from the built-in constants plus NVS
    /// overrides (no file layer — `resolve_in` covers the full stack).
    /// Host convenience; on device pass a `StaticCell` scratch instead.
    pub fn resolve_nvs(
        overrides: &dyn OverrideSource,
    ) -> Result<(Self, Warnings), ConfigError> {
        let mut scratch = Table::new();
        Self::resolve_in(None, overrides, &mut scratch)
    }

    /// Resolves the full stack — built-in constants, then `file` (the
    /// committed defaults, or a deployment's variant), then NVS
    /// overrides — filling `scratch` for the parse. Warnings accumulate
    /// across all layers.
    pub fn resolve_in(
        file: Option<&str>,
        overrides: &dyn OverrideSource,
        scratch: &mut Table,
    ) -> Result<(Self, Warnings), ConfigError> {
        let mut config = Self::BUILT_IN;
        let mut warnings = Warnings::new();

        if let Some(text) = file {
            toml::parse(text, KNOWN_TABLES, scratch)?;
            warnings.merge(scratch.warnings());
            for entry in scratch.iter() {
                if !config.set_field(entry.key.as_str(), &entry.value)? {
                    warnings.push(Warning {
                        kind: WarningKind::UnknownKey,
                        path: entry.key,
                    });
                }
            }
        }

        config.apply_overrides(overrides, &mut warnings);
        Ok((config, warnings))
    }

    /// Applies one known-key entry to the matching field. Returns `false`
    /// for unknown paths (the caller warns); `Err` for a known key whose
    /// value is wrong for the field.
    fn set_field(&mut self, path: &str, v: &Value) -> Result<bool, ConfigError> {
        // Tripwire: secret keys are NVS-only and must never gain a config
        // field. If a refactor ever routes one here, debug builds stop.
        debug_assert!(
            !crate::nvs::SECRET_KEYS.contains(&path),
            "a secret key reached the config layer"
        );
        match path {
            "sensor.id" => {
                let s = value_str(v, "sensor.id")?;
                if !is_uuid_shape(s) {
                    return Err(bad("sensor.id"));
                }
                self.sensor.id.set(s);
            }
            "sampling.decibel_window_ms" => {
                let ms = value_u32(v, 125, 3_600, "sampling.decibel_window_ms")?;
                if ms % FAST_WINDOW_MS != 0 {
                    // decibel.md: Fast windows (125 ms) fold inside the
                    // aggregate window — a non-multiple cannot be honored.
                    return Err(bad("sampling.decibel_window_ms"));
                }
                self.sampling.decibel_window_ms = ms;
            }
            "sampling.report_interval_s" => {
                self.sampling.report_interval_s =
                    value_u32(v, 1, 86_400, "sampling.report_interval_s")?;
            }
            "sampling.offline_buffer" => {
                self.sampling.offline_buffer =
                    value_u32(v, 1, 4_096, "sampling.offline_buffer")?;
            }
            "sensors.bme280.i2c_address" => {
                self.sensors.bme280.i2c_address =
                    value_i2c_addr(v, "sensors.bme280.i2c_address")?;
            }
            "sensors.sht4x.i2c_address" => {
                self.sensors.sht4x.i2c_address =
                    value_i2c_addr(v, "sensors.sht4x.i2c_address")?;
            }
            "sensors.scd4x.i2c_address" => {
                self.sensors.scd4x.i2c_address =
                    value_i2c_addr(v, "sensors.scd4x.i2c_address")?;
            }
            "sensors.bh1750.i2c_address" => {
                self.sensors.bh1750.i2c_address =
                    value_i2c_addr(v, "sensors.bh1750.i2c_address")?;
            }
            "sensors.pir.gpio_pin" => {
                self.sensors.pir.gpio_pin = value_u8(v, 0, 48, "sensors.pir.gpio_pin")?;
            }
            "sensors.pms5003.uart_baud" => {
                self.sensors.pms5003.uart_baud =
                    value_u32(v, 1_200, 1_000_000, "sensors.pms5003.uart_baud")?;
            }
            "transport.active" => {
                let s = value_str(v, "transport.active")?;
                self.transport.active =
                    TransportKind::from_name(s).ok_or_else(|| bad("transport.active"))?;
            }
            "transport.http_endpoint" => {
                let s = value_str(v, "transport.http_endpoint")?;
                self.transport.http_endpoint = value_fits(s, "transport.http_endpoint")?;
            }
            "transport.mqtt.broker" => {
                let s = value_str(v, "transport.mqtt.broker")?;
                self.transport.mqtt.broker = value_fits(s, "transport.mqtt.broker")?;
            }
            "transport.mqtt.port" => {
                self.transport.mqtt.port =
                    value_u16(v, 1, 65_535, "transport.mqtt.port")?;
            }
            "decibel.overload_threshold_db" => {
                self.decibel.overload_threshold_db =
                    value_f32(v, 0.0, 130.0, "decibel.overload_threshold_db")?;
            }
            "decibel.calibration_offset_db" => {
                self.decibel.calibration_offset_db =
                    value_f32(v, -120.0, 120.0, "decibel.calibration_offset_db")?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Layers every tunable the source carries onto this config. Only
    /// [`KNOWN_PATHS`] are consulted, so source entries outside the
    /// schema — including every secret key — cannot reach a `Config`.
    fn apply_overrides(&mut self, src: &dyn OverrideSource, warnings: &mut Warnings) {
        for path in KNOWN_PATHS {
            let mut buf = [0u8; MAX_VALUE_LEN + 1];
            let read = match src.get(path, &mut buf) {
                Ok(Some(n)) => n,
                Ok(None) => continue,
                Err(_) => {
                    warnings.push(Warning {
                        kind: WarningKind::OverrideReadFailed,
                        path: warn_path(path),
                    });
                    continue;
                }
            };
            let text = match core::str::from_utf8(&buf[..read]) {
                Ok(t) => t,
                Err(_) => {
                    warnings.push(Warning {
                        kind: WarningKind::BadOverride,
                        path: warn_path(path),
                    });
                    continue;
                }
            };
            let value = match toml::parse_value(text) {
                Ok(v) => v,
                Err(_) => {
                    warnings.push(Warning {
                        kind: WarningKind::BadOverride,
                        path: warn_path(path),
                    });
                    continue;
                }
            };
            if !self.set_field(path, &value).unwrap_or(false) {
                warnings.push(Warning {
                    kind: WarningKind::BadOverride,
                    path: warn_path(path),
                });
            }
        }
    }

    /// Renders the config as canonical TOML — every known path under its
    /// table — into `out`, returning the byte count. Used by the
    /// round-trip test, the status page, and any debug dump: what this
    /// writes is provably secret-free (see the conformance tests).
    pub fn write_toml(&self, out: &mut [u8]) -> Result<usize, RenderError> {
        let mut w = TomlWriter::new(out);
        let c = self;

        w.table("[sensor]")?;
        w.key_str("id", c.sensor.id.as_str())?;
        w.blank()?;

        w.table("[sampling]")?;
        w.key_int("decibel_window_ms", c.sampling.decibel_window_ms)?;
        w.key_int("report_interval_s", c.sampling.report_interval_s)?;
        w.key_int("offline_buffer", c.sampling.offline_buffer)?;
        w.blank()?;

        w.table("[sensors.bme280]")?;
        w.key_hex("i2c_address", c.sensors.bme280.i2c_address)?;
        w.blank()?;
        w.table("[sensors.sht4x]")?;
        w.key_hex("i2c_address", c.sensors.sht4x.i2c_address)?;
        w.blank()?;
        w.table("[sensors.scd4x]")?;
        w.key_hex("i2c_address", c.sensors.scd4x.i2c_address)?;
        w.blank()?;
        w.table("[sensors.bh1750]")?;
        w.key_hex("i2c_address", c.sensors.bh1750.i2c_address)?;
        w.blank()?;
        w.table("[sensors.pir]")?;
        w.key_int("gpio_pin", c.sensors.pir.gpio_pin)?;
        w.blank()?;
        w.table("[sensors.pms5003]")?;
        w.key_int("uart_baud", c.sensors.pms5003.uart_baud)?;
        w.blank()?;

        w.table("[transport]")?;
        w.key_str("active", c.transport.active.as_str())?;
        w.key_str("http_endpoint", c.transport.http_endpoint.as_str())?;
        w.blank()?;

        w.table("[transport.mqtt]")?;
        w.key_str("broker", c.transport.mqtt.broker.as_str())?;
        w.key_int("port", c.transport.mqtt.port)?;
        w.blank()?;

        w.table("[decibel]")?;
        w.key_float("overload_threshold_db", c.decibel.overload_threshold_db)?;
        w.key_float("calibration_offset_db", c.decibel.calibration_offset_db)?;

        Ok(w.pos())
    }
}

/// Why rendering into a buffer failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The destination buffer cannot hold the canonical TOML. Size it
    /// from [`RENDER_CAP`].
    BufferFull,
}

/// Suggested buffer for [`Config::write_toml`] (the committed defaults
/// render to ~450 bytes; leave headroom for long endpoints).
pub const RENDER_CAP: usize = 1_024;

// ---------------------------------------------------------------------------
// Typed extraction helpers — each known key reads through exactly one of
// these, so a wrong-typed file entry is always a `BadValue` error.
// ---------------------------------------------------------------------------

fn value_u32(
    v: &Value,
    min: u32,
    max: u32,
    path: &'static str,
) -> Result<u32, ConfigError> {
    let i = v.as_i64().ok_or_else(|| bad(path))?;
    if i < min as i64 || i > max as i64 {
        return Err(bad(path));
    }
    Ok(i as u32)
}

fn value_u16(
    v: &Value,
    min: u16,
    max: u16,
    path: &'static str,
) -> Result<u16, ConfigError> {
    let i = v.as_i64().ok_or_else(|| bad(path))?;
    if i < min as i64 || i > max as i64 {
        return Err(bad(path));
    }
    Ok(i as u16)
}

fn value_u8(v: &Value, min: u8, max: u8, path: &'static str) -> Result<u8, ConfigError> {
    let i = v.as_i64().ok_or_else(|| bad(path))?;
    if i < min as i64 || i > max as i64 {
        return Err(bad(path));
    }
    Ok(i as u8)
}

fn value_i2c_addr(v: &Value, path: &'static str) -> Result<u8, ConfigError> {
    // 7-bit I2C address space; 0x00–0x07 and 0x78–0x7F are reserved.
    let i = v.as_i64().ok_or_else(|| bad(path))?;
    if !(0x08..=0x77).contains(&i) {
        return Err(bad(path));
    }
    Ok(i as u8)
}

fn value_f32(
    v: &Value,
    min: f32,
    max: f32,
    path: &'static str,
) -> Result<f32, ConfigError> {
    let f = v.as_f32().ok_or_else(|| bad(path))?;
    if f < min || f > max {
        return Err(bad(path));
    }
    Ok(f)
}

fn value_str<'a>(v: &'a Value, path: &'static str) -> Result<&'a str, ConfigError> {
    v.as_str().ok_or_else(|| bad(path))
}

/// Copies a string into a `FixedStr` of the field's capacity.
fn value_fits<const N: usize>(
    s: &str,
    path: &'static str,
) -> Result<FixedStr<N>, ConfigError> {
    FixedStr::from_str(s).ok_or(ConfigError::ValueTooLong { path })
}

/// Every call site passes a `KNOWN_PATHS` element or a match-arm literal,
/// both `&'static str`, so `BadValue` can carry the key name.
fn bad(path: &str) -> ConfigError {
    ConfigError::BadValue {
        path: path_of(path),
    }
}

/// Resolves a `&str` back to its `KNOWN_PATHS` entry — the only strings
/// reaching [`bad`] — keeping `BadValue`'s payload truly `'static` without
/// an unsafe cast.
fn path_of(path: &str) -> &'static str {
    for known in KNOWN_PATHS {
        if *known == path {
            return known;
        }
    }
    "unknown-path"
}

fn warn_path(path: &str) -> FixedStr<MAX_KEY_LEN> {
    FixedStr::from_str(path).unwrap_or_default()
}

/// The `sensor.id` value must at least be UUID-shaped: 36 bytes, dashes
/// at the `8-4-4-4-12` positions, hex elsewhere. It is a placeholder by
/// design — the real identity is provisioned per device.
fn is_uuid_shape(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != UUID_LEN {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        let dashed = i == 8 || i == 13 || i == 18 || i == 23;
        if dashed {
            if c != b'-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Canonical TOML rendering
// ---------------------------------------------------------------------------

/// A `core::fmt::Write` adapter over a fixed byte buffer.
struct TomlWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> TomlWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn push(&mut self, s: &str) -> Result<(), RenderError> {
        let rest = self
            .buf
            .get_mut(self.pos..)
            .ok_or(RenderError::BufferFull)?;
        let bytes = s.as_bytes();
        if bytes.len() > rest.len() {
            return Err(RenderError::BufferFull);
        }
        rest[..bytes.len()].copy_from_slice(bytes);
        self.pos += bytes.len();
        Ok(())
    }

    fn table(&mut self, name: &str) -> Result<(), RenderError> {
        self.push(name)?;
        self.push("\n")
    }

    fn blank(&mut self) -> Result<(), RenderError> {
        self.push("\n")
    }

    fn key_str(&mut self, key: &str, value: &str) -> Result<(), RenderError> {
        self.push(key)?;
        self.push(" = \"")?;
        self.push_escaped(value)?;
        self.push("\"\n")
    }

    fn key_int(
        &mut self,
        key: &str,
        value: impl core::fmt::Display,
    ) -> Result<(), RenderError> {
        self.push_key_val(key)?;
        write!(self, "{value}").map_err(|_| RenderError::BufferFull)?;
        self.push("\n")
    }

    fn key_hex(&mut self, key: &str, value: u8) -> Result<(), RenderError> {
        self.push_key_val(key)?;
        write!(self, "0x{value:02X}").map_err(|_| RenderError::BufferFull)?;
        self.push("\n")
    }

    /// Floats always render a fraction part — TOML requires it, and it
    /// keeps the canonical form stable for the round-trip test. (The
    /// pinned fork exposes no `f32::fract`; the integral test is a
    /// round-trip cast.)
    fn key_float(&mut self, key: &str, value: f32) -> Result<(), RenderError> {
        self.push_key_val(key)?;
        if value.is_finite() && value == value as i64 as f32 {
            // `{}` would drop the fraction (`120` is an int in TOML).
            write!(self, "{value:.1}").map_err(|_| RenderError::BufferFull)?;
        } else {
            write!(self, "{value}").map_err(|_| RenderError::BufferFull)?;
        }
        self.push("\n")
    }

    fn push_key_val(&mut self, key: &str) -> Result<(), RenderError> {
        self.push(key)?;
        self.push(" = ")
    }

    /// Pushes `s` with the basic escapes applied (`\"`, `\\`, `\n`, `\r`,
    /// `\t`).
    fn push_escaped(&mut self, s: &str) -> Result<(), RenderError> {
        let mut start = 0;
        for (i, c) in s.char_indices() {
            let esc: &str = match c {
                '"' => "\\\"",
                '\\' => "\\\\",
                '\n' => "\\n",
                '\r' => "\\r",
                '\t' => "\\t",
                _ => continue,
            };
            self.push(&s[start..i])?;
            self.push(esc)?;
            start = i + c.len_utf8();
        }
        self.push(&s[start..])
    }
}

impl core::fmt::Write for TomlWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.push(s).map_err(|_| core::fmt::Error)
    }
}
