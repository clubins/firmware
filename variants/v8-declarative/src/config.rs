//! The device configuration: one committed `config/default.toml` of
//! runtime tunables, resolved through a strict precedence ladder.
//!
//! Precedence is one rule, tested on the host (spec §Configuration):
//!
//! ```text
//! NVS value  >  config/default.toml value  >  built-in constant
//! ```
//!
//! [`Config::parse`] starts from the built-in constants and applies the
//! document over them, so a partial document still yields a complete
//! [`Config`] (and an empty one yields the built-ins verbatim); the NVS
//! layer ([`crate::nvs`]) then applies device overrides on top. The two
//! layers have deliberately different failure posture:
//!
//! - The committed document is developer-edited and CI-reviewed, so a
//!   known key with a bad type or range fails the boot loudly — the same
//!   philosophy as `pipelines.toml`. An unknown key is only warned about:
//!   a config written for a newer firmware must still boot here.
//! - NVS state is written by provisioning onto devices nobody reviews, so
//!   nothing read from it aborts the boot — unknown keys and unusable
//!   values are warned about and the lower-precedence value stands.
//!
//! Secrets (Wi-Fi credentials, the API key, the client certificate and
//! key) never appear in this file or in any struct below: the schema has
//! no field that could hold one. [`SECRET_MARKERS`] plus
//! [`assert_no_secret_markers`] — debug-asserted on every serialization —
//! keep it that way.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use core::fmt;
use toml::de::{DeTable, DeValue};

/// Key-name fragments that identify secret material. No `Config` field may
/// ever serialize a key containing any of these — secrets live only in NVS
/// (spec §Configuration). [`Config::to_toml`] debug-asserts the absence of
/// every marker on every call; the host tests exercise the tripwire with
/// real secret values in the override source.
pub const SECRET_MARKERS: &[&str] = &[
    "password",
    "passphrase",
    "secret",
    "psk",
    "api_key",
    "apikey",
    "ssid",
    "client_cert",
    "client_key",
    "private_key",
];

/// Whether `text` mentions any [`SECRET_MARKERS`]. Public so the NVS tests
/// can cross-check that every secret NVS key would be caught.
pub fn serialized_output_contains_secret_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    SECRET_MARKERS.iter().any(|m| lower.contains(m))
}

/// The serialization tripwire: in debug builds a serialized config that
/// mentions secret material panics instead of shipping it to a log.
fn assert_no_secret_markers(text: &str) {
    debug_assert!(
        !serialized_output_contains_secret_marker(text),
        "serialized config mentions secret material — a Config field gained \
         a secret-named field; secrets live only in NVS"
    );
}

/// Non-fatal notes gathered while parsing or resolving a config: unknown
/// keys tolerated for forward compatibility, NVS entries skipped, and so
/// on. All of them are rendered at boot so a tolerated typo is still
/// visible to the operator.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Warnings(Vec<String>);

impl Warnings {
    pub fn push(&mut self, note: String) {
        self.0.push(note);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl fmt::Display for Warnings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for note in &self.0 {
            writeln!(f, "warning: {note}")?;
        }
        Ok(())
    }
}

/// A parsed device config plus the warnings its parse produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub config: Config,
    pub warnings: Warnings,
}

/// Which transport the device reports through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportKind {
    Http,
    Mqtt,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportKind::Http => "http",
            TransportKind::Mqtt => "mqtt",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "http" => Some(TransportKind::Http),
            "mqtt" => Some(TransportKind::Mqtt),
            _ => None,
        }
    }
}

impl fmt::Display for TransportKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Device identity.
#[derive(Clone, Debug, PartialEq)]
pub struct SensorConfig {
    /// The sensor UUID. The placeholder here is overwritten from NVS by
    /// provisioning; the real UUID never appears in a committed file.
    pub id: String,
}

/// Sampling and buffering budgets (not per-pipeline cadences — those are
/// declared in `config/pipelines.toml`).
#[derive(Clone, Debug, PartialEq)]
pub struct SamplingConfig {
    /// Outer decibel report window; Fast windows (125 ms) live inside it.
    pub decibel_window_ms: u32,
    /// Per-type reporting cadence budget.
    pub report_interval_s: u32,
    /// Measurements held while disconnected, flushed on reconnect.
    pub offline_buffer: u32,
}

/// Per-sensor hardware addresses.
#[derive(Clone, Debug, PartialEq)]
pub struct SensorsConfig {
    pub bme280: Bme280Config,
    pub pir: PirConfig,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bme280Config {
    /// 7-bit I2C address.
    pub i2c_address: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PirConfig {
    /// GPIO the PIR front-end watches (ESP32-S3: 0..=48).
    pub gpio_pin: u8,
}

/// Transport selection and endpoints.
#[derive(Clone, Debug, PartialEq)]
pub struct TransportConfig {
    /// The transport that runs. Both may be compiled in; this selects.
    pub active: TransportKind,
    pub http_endpoint: String,
    pub mqtt: MqttConfig,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MqttConfig {
    pub broker: String,
    /// TLS listener; plain 1883 only without mTLS.
    pub port: u16,
}

/// Decibel DSP tunables.
#[derive(Clone, Debug, PartialEq)]
pub struct DecibelConfig {
    /// Level at which the DSP flags an overload.
    pub overload_threshold_db: f64,
    /// Per-unit calibration arrives from NVS; this is the neutral default.
    pub calibration_offset_db: f64,
}

/// The complete device configuration: every field is filled at every rung
/// of the precedence ladder, so a consumer never sees an "unset" value.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub sensor: SensorConfig,
    pub sampling: SamplingConfig,
    pub sensors: SensorsConfig,
    pub transport: TransportConfig,
    pub decibel: DecibelConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self::builtin()
    }
}

impl Config {
    /// The built-in constants — the last rung of the precedence ladder.
    /// They carry the same canonical values as the committed document:
    /// this rung is what a document-less or partial parse stands on.
    pub fn builtin() -> Self {
        Self {
            sensor: SensorConfig {
                id: "00000000-0000-0000-0000-000000000000".into(),
            },
            sampling: SamplingConfig {
                decibel_window_ms: 1000,
                report_interval_s: 60,
                offline_buffer: 64,
            },
            sensors: SensorsConfig {
                bme280: Bme280Config { i2c_address: 0x76 },
                pir: PirConfig { gpio_pin: 4 },
            },
            transport: TransportConfig {
                active: TransportKind::Http,
                http_endpoint: "https://ingest.clubins.de/v1/sensors".into(),
                mqtt: MqttConfig {
                    broker: "mqtt.clubins.de".into(),
                    port: 8883,
                },
            },
            decibel: DecibelConfig {
                overload_threshold_db: 120.0,
                calibration_offset_db: 0.0,
            },
        }
    }

    /// Parse a device config document over the built-in constants.
    ///
    /// Unknown sections and unknown keys inside known sections are
    /// tolerated with a warning (forward compatibility); a known key with
    /// a wrong type or an out-of-range value is a hard error, because this
    /// document is committed and CI-reviewed — a silent mis-tune here
    /// would ship to every device.
    pub fn parse(src: &str) -> Result<Parsed, ConfigError> {
        let spanned =
            DeTable::parse(src).map_err(|e| ConfigError::Syntax(e.to_string()))?;
        let mut doc = spanned.into_inner();
        doc.make_owned();

        let mut warnings = Warnings::default();
        let mut config = Self::builtin();

        for (key, value) in doc.iter() {
            let section = key.get_ref().as_ref();
            let table = match value.get_ref().as_table() {
                Some(t) => t,
                None => {
                    return Err(ConfigError::WrongShape {
                        ctx: alloc::format!("[{section}]"),
                        why: "section must be a table".into(),
                    });
                }
            };
            match section {
                "sensor" => parse_sensor(table, &mut config.sensor, &mut warnings)?,
                "sampling" => parse_sampling(table, &mut config.sampling, &mut warnings)?,
                "sensors" => parse_sensors(table, &mut config.sensors, &mut warnings)?,
                "transport" => {
                    parse_transport(table, &mut config.transport, &mut warnings)?
                }
                "decibel" => parse_decibel(table, &mut config.decibel, &mut warnings)?,
                other => warnings.push(format!(
                    "default.toml: unknown section '[{other}]' — ignored"
                )),
            }
        }

        Ok(Parsed { config, warnings })
    }

    /// Serialize the config back to canonical TOML — the round-trip half
    /// of the parse, and the text whose secret-freedom is debug-asserted.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();

        out.push_str("[sensor]\n");
        out.push_str(&format!("id = \"{}\"\n", escape(&self.sensor.id)));

        out.push_str("\n[sampling]\n");
        out.push_str(&format!(
            "decibel_window_ms = {}\n",
            self.sampling.decibel_window_ms
        ));
        out.push_str(&format!(
            "report_interval_s = {}\n",
            self.sampling.report_interval_s
        ));
        out.push_str(&format!(
            "offline_buffer = {}\n",
            self.sampling.offline_buffer
        ));

        out.push_str("\n[sensors.bme280]\n");
        out.push_str(&format!(
            "i2c_address = {:#x}\n",
            self.sensors.bme280.i2c_address
        ));

        out.push_str("\n[sensors.pir]\n");
        out.push_str(&format!("gpio_pin = {}\n", self.sensors.pir.gpio_pin));

        out.push_str("\n[transport]\n");
        out.push_str(&format!(
            "active = \"{}\"\n",
            self.transport.active.as_str()
        ));
        out.push_str(&format!(
            "http_endpoint = \"{}\"\n",
            escape(&self.transport.http_endpoint)
        ));

        out.push_str("\n[transport.mqtt]\n");
        out.push_str(&format!(
            "broker = \"{}\"\n",
            escape(&self.transport.mqtt.broker)
        ));
        out.push_str(&format!("port = {}\n", self.transport.mqtt.port));

        out.push_str("\n[decibel]\n");
        out.push_str(&format!(
            "overload_threshold_db = {}\n",
            fmt_f64(self.decibel.overload_threshold_db)
        ));
        out.push_str(&format!(
            "calibration_offset_db = {}\n",
            fmt_f64(self.decibel.calibration_offset_db)
        ));

        assert_no_secret_markers(&out);
        out
    }

    /// Whether `active`'s cargo feature is compiled into this binary.
    /// Selecting a transport is a config edit; enabling one that was not
    /// compiled in is not — the boot checks this loudly. A config edit
    /// cannot smuggle in uncompiled code (spec §Configuration).
    pub fn is_compiled(active: TransportKind) -> bool {
        match active {
            TransportKind::Http => cfg!(feature = "transport-http"),
            TransportKind::Mqtt => cfg!(feature = "transport-mqtt"),
        }
    }
}

/// A failure turning `config/default.toml` text into a [`Config`]. The
/// committed document is strict: every variant aborts the boot.
#[derive(Debug, PartialEq)]
pub enum ConfigError {
    /// The text is not valid TOML.
    Syntax(String),
    /// A section that must be a table was not.
    WrongShape { ctx: String, why: String },
    /// A key was present but has the wrong value type.
    BadType {
        ctx: String,
        field: String,
        want: &'static str,
    },
    /// A key was present with the right type but an unusable value.
    BadValue {
        ctx: String,
        field: String,
        why: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Syntax(e) => write!(f, "default.toml is not valid TOML: {e}"),
            ConfigError::WrongShape { ctx, why } => write!(f, "{ctx}: {why}"),
            ConfigError::BadType { ctx, field, want } => {
                write!(f, "{ctx}: '{field}' must be {want}")
            }
            ConfigError::BadValue { ctx, field, why } => {
                write!(f, "{ctx}: '{field}' {why}")
            }
        }
    }
}

fn parse_sensor(
    table: &DeTable<'_>,
    cfg: &mut SensorConfig,
    warnings: &mut Warnings,
) -> Result<(), ConfigError> {
    let ctx = "[sensor]";
    for (key, value) in table.iter() {
        let field = key.get_ref().as_ref();
        match field {
            "id" => {
                let s = want_str(value.get_ref(), ctx, "id")?;
                if !is_uuid_like(&s) {
                    return Err(ConfigError::BadValue {
                        ctx: ctx.into(),
                        field: "id".into(),
                        why: "must be a UUID (8-4-4-4-12 hex)".into(),
                    });
                }
                cfg.id = s;
            }
            other => push_unknown_key(warnings, ctx, other),
        }
    }
    Ok(())
}

fn parse_sampling(
    table: &DeTable<'_>,
    cfg: &mut SamplingConfig,
    warnings: &mut Warnings,
) -> Result<(), ConfigError> {
    let ctx = "[sampling]";
    for (key, value) in table.iter() {
        let field = key.get_ref().as_ref();
        match field {
            "decibel_window_ms" => {
                cfg.decibel_window_ms = want_u64(
                    value.get_ref(),
                    ctx,
                    "decibel_window_ms",
                    1,
                    u32::MAX as i64,
                )? as u32;
            }
            "report_interval_s" => {
                cfg.report_interval_s = want_u64(
                    value.get_ref(),
                    ctx,
                    "report_interval_s",
                    1,
                    u32::MAX as i64,
                )? as u32;
            }
            "offline_buffer" => {
                cfg.offline_buffer =
                    want_u64(value.get_ref(), ctx, "offline_buffer", 0, u32::MAX as i64)?
                        as u32;
            }
            other => push_unknown_key(warnings, ctx, other),
        }
    }
    Ok(())
}

fn parse_sensors(
    table: &DeTable<'_>,
    cfg: &mut SensorsConfig,
    warnings: &mut Warnings,
) -> Result<(), ConfigError> {
    for (key, value) in table.iter() {
        let sensor = key.get_ref().as_ref();
        let sub = match value.get_ref().as_table() {
            Some(t) => t,
            None => {
                return Err(ConfigError::WrongShape {
                    ctx: alloc::format!("[sensors.{sensor}]"),
                    why: "sensor section must be a table".into(),
                });
            }
        };
        match sensor {
            "bme280" => {
                for (k, v) in sub.iter() {
                    if k.get_ref().as_ref() == "i2c_address" {
                        cfg.bme280.i2c_address = want_u64(
                            v.get_ref(),
                            "[sensors.bme280]",
                            "i2c_address",
                            0,
                            0x7f,
                        )? as u8;
                    } else {
                        push_unknown_key(
                            warnings,
                            "[sensors.bme280]",
                            k.get_ref().as_ref(),
                        );
                    }
                }
            }
            "pir" => {
                for (k, v) in sub.iter() {
                    if k.get_ref().as_ref() == "gpio_pin" {
                        cfg.pir.gpio_pin =
                            want_u64(v.get_ref(), "[sensors.pir]", "gpio_pin", 0, 48)?
                                as u8;
                    } else {
                        push_unknown_key(warnings, "[sensors.pir]", k.get_ref().as_ref());
                    }
                }
            }
            other => warnings.push(format!(
                "default.toml: unknown sensor section '[sensors.{other}]' — ignored"
            )),
        }
    }
    Ok(())
}

fn parse_transport(
    table: &DeTable<'_>,
    cfg: &mut TransportConfig,
    warnings: &mut Warnings,
) -> Result<(), ConfigError> {
    let ctx = "[transport]";
    for (key, value) in table.iter() {
        let field = key.get_ref().as_ref();
        match field {
            "active" => {
                let s = want_str(value.get_ref(), ctx, "active")?;
                cfg.active =
                    TransportKind::parse(&s).ok_or_else(|| ConfigError::BadValue {
                        ctx: ctx.into(),
                        field: "active".into(),
                        why: "must be \"http\" or \"mqtt\"".into(),
                    })?;
            }
            "http_endpoint" => {
                let s = want_str(value.get_ref(), ctx, "http_endpoint")?;
                if s.is_empty() {
                    return Err(ConfigError::BadValue {
                        ctx: ctx.into(),
                        field: "http_endpoint".into(),
                        why: "must not be empty".into(),
                    });
                }
                cfg.http_endpoint = s;
            }
            "mqtt" => {
                let sub = value.get_ref().as_table().ok_or_else(|| {
                    ConfigError::WrongShape {
                        ctx: "[transport.mqtt]".into(),
                        why: "section must be a table".into(),
                    }
                })?;
                for (k, v) in sub.iter() {
                    let mqtt_field = k.get_ref().as_ref();
                    match mqtt_field {
                        "broker" => {
                            let s = want_str(v.get_ref(), "[transport.mqtt]", "broker")?;
                            if s.is_empty() {
                                return Err(ConfigError::BadValue {
                                    ctx: "[transport.mqtt]".into(),
                                    field: "broker".into(),
                                    why: "must not be empty".into(),
                                });
                            }
                            cfg.mqtt.broker = s;
                        }
                        "port" => {
                            cfg.mqtt.port = want_u64(
                                v.get_ref(),
                                "[transport.mqtt]",
                                "port",
                                1,
                                65535,
                            )? as u16;
                        }
                        other => push_unknown_key(warnings, "[transport.mqtt]", other),
                    }
                }
            }
            other => push_unknown_key(warnings, ctx, other),
        }
    }
    Ok(())
}

fn parse_decibel(
    table: &DeTable<'_>,
    cfg: &mut DecibelConfig,
    warnings: &mut Warnings,
) -> Result<(), ConfigError> {
    let ctx = "[decibel]";
    for (key, value) in table.iter() {
        let field = key.get_ref().as_ref();
        match field {
            "overload_threshold_db" => {
                cfg.overload_threshold_db =
                    want_f64(value.get_ref(), ctx, "overload_threshold_db", 0.0, 200.0)?;
            }
            "calibration_offset_db" => {
                cfg.calibration_offset_db = want_f64(
                    value.get_ref(),
                    ctx,
                    "calibration_offset_db",
                    -120.0,
                    120.0,
                )?;
            }
            other => push_unknown_key(warnings, ctx, other),
        }
    }
    Ok(())
}

// ——— typed lookups ————————————————————————————————————————————————

/// Tolerated-but-reported: an unknown key (a typo, or a field written by a
/// newer firmware) is skipped and the operator sees why at boot.
fn push_unknown_key(warnings: &mut Warnings, ctx: &str, field: &str) {
    warnings.push(format!(
        "default.toml: unknown key '{ctx}.{field}' — ignored, the built-in value stands"
    ));
}

/// Extract a string value from an already-present key.
fn want_str(value: &DeValue<'_>, ctx: &str, field: &str) -> Result<String, ConfigError> {
    value
        .as_str()
        .map(String::from)
        .ok_or_else(|| ConfigError::BadType {
            ctx: ctx.into(),
            field: field.into(),
            want: "a string",
        })
}

/// Extract an integer value (TOML hex/binary/decimal forms all land here),
/// range-checked.
fn want_u64(
    value: &DeValue<'_>,
    ctx: &str,
    field: &str,
    min: i64,
    max: i64,
) -> Result<u64, ConfigError> {
    let DeValue::Integer(i) = value else {
        return Err(ConfigError::BadType {
            ctx: ctx.into(),
            field: field.into(),
            want: "an integer",
        });
    };
    let raw = i64::from_str_radix(i.as_str(), i.radix()).map_err(|_| {
        ConfigError::BadValue {
            ctx: ctx.into(),
            field: field.into(),
            why: "is not a representable integer".into(),
        }
    })?;
    if raw < min || raw > max {
        return Err(ConfigError::BadValue {
            ctx: ctx.into(),
            field: field.into(),
            why: format!("must be in {min}..={max}"),
        });
    }
    Ok(raw as u64)
}

/// Extract a float value (an integer literal is accepted as a float),
/// range-checked.
fn want_f64(
    value: &DeValue<'_>,
    ctx: &str,
    field: &str,
    min: f64,
    max: f64,
) -> Result<f64, ConfigError> {
    let parsed: f64 = match value {
        DeValue::Float(fl) => fl.as_str().parse().map_err(|_| ConfigError::BadValue {
            ctx: ctx.into(),
            field: field.into(),
            why: "is not a representable float".into(),
        })?,
        DeValue::Integer(i) => {
            i64::from_str_radix(i.as_str(), i.radix()).map_err(|_| {
                ConfigError::BadValue {
                    ctx: ctx.into(),
                    field: field.into(),
                    why: "is not a representable integer".into(),
                }
            })? as f64
        }
        _ => {
            return Err(ConfigError::BadType {
                ctx: ctx.into(),
                field: field.into(),
                want: "a number",
            });
        }
    };
    if !parsed.is_finite() || parsed < min || parsed > max {
        return Err(ConfigError::BadValue {
            ctx: ctx.into(),
            field: field.into(),
            why: format!("must be in {min}..={max}"),
        });
    }
    Ok(parsed)
}

/// Whether `s` is shaped like a UUID: 8-4-4-4-12 hex with dashes at the
/// fixed positions. Used by the committed-document parse and (for the
/// same reason) by the NVS applier.
pub fn is_uuid_like(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if *b != b'-' {
                    return false;
                }
            }
            _ => {
                if !b.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

/// Escape a string for a basic TOML string literal (override-provided
/// values may carry text that must not break the document).
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// Format a float so TOML still reads it as a float (`120`, the default
/// `Display` rendering, would parse back as an integer).
fn fmt_f64(v: f64) -> String {
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{s}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed document — parsed from the crate's own config/.
    fn committed() -> Parsed {
        Config::parse(include_str!("../config/default.toml"))
            .expect("committed default.toml parses")
    }

    #[test]
    fn committed_document_parses_to_spec_defaults() {
        let Parsed { config, warnings } = committed();
        assert!(warnings.is_empty(), "committed file must parse clean");
        assert_eq!(config.sensor.id, "00000000-0000-0000-0000-000000000000");
        assert_eq!(config.sampling.decibel_window_ms, 1000);
        assert_eq!(config.sampling.report_interval_s, 60);
        assert_eq!(config.sampling.offline_buffer, 64);
        assert_eq!(config.sensors.bme280.i2c_address, 0x76);
        assert_eq!(config.sensors.pir.gpio_pin, 4);
        assert_eq!(config.transport.active, TransportKind::Http);
        assert_eq!(
            config.transport.http_endpoint,
            "https://ingest.clubins.de/v1/sensors"
        );
        assert_eq!(config.transport.mqtt.broker, "mqtt.clubins.de");
        assert_eq!(config.transport.mqtt.port, 8883);
        assert_eq!(config.decibel.overload_threshold_db, 120.0);
        assert_eq!(config.decibel.calibration_offset_db, 0.0);
    }

    #[test]
    fn empty_document_yields_builtin_constants() {
        let Parsed { config, warnings } = Config::parse("").expect("empty parses");
        assert!(warnings.is_empty());
        assert_eq!(config, Config::builtin());
    }

    #[test]
    fn partial_documents_fill_from_builtins() {
        let Parsed { config, warnings } = Config::parse(
            r#"
            [decibel]
            calibration_offset_db = -2.5
            "#,
        )
        .expect("partial parses");
        assert!(warnings.is_empty());
        assert_eq!(
            config,
            Config {
                decibel: DecibelConfig {
                    calibration_offset_db: -2.5,
                    ..Config::builtin().decibel
                },
                ..Config::builtin()
            }
        );
    }

    #[test]
    fn unknown_section_is_tolerated_with_warning() {
        let Parsed { config, warnings } = Config::parse(
            r#"
            [samplng]
            report_interval_s = 30

            [decibel]
            overload_threshold_db = 118.0
            "#,
        )
        .expect("unknown section tolerated");
        // The typo'd section is ignored; everything else still applies.
        assert_eq!(config.sampling.report_interval_s, 60);
        assert_eq!(config.decibel.overload_threshold_db, 118.0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings.iter().next().unwrap().contains("[samplng]"));
    }

    #[test]
    fn unknown_field_in_known_section_is_tolerated_with_warning() {
        let Parsed { config, warnings } = Config::parse(
            r#"
            [sampling]
            report_intervall = 30
            "#,
        )
        .expect("unknown key tolerated");
        assert_eq!(config.sampling.report_interval_s, 60);
        assert_eq!(warnings.len(), 1);
        assert!(warnings.iter().next().unwrap().contains("report_intervall"));
    }

    #[test]
    fn unknown_sensor_subtable_is_tolerated() {
        let Parsed { config, warnings } = Config::parse(
            r#"
            [sensors.scd4x]
            i2c_address = 0x62
            "#,
        )
        .expect("unknown sensor section tolerated");
        assert_eq!(config.sensors.bme280.i2c_address, 0x76);
        assert_eq!(warnings.len(), 1);
        assert!(warnings.iter().next().unwrap().contains("scd4x"));
    }

    #[test]
    fn wrong_type_on_known_key_is_a_hard_error() {
        let err = Config::parse(
            r#"
            [sampling]
            report_interval_s = "60"
            "#,
        )
        .expect_err("string where integer is required");
        assert!(
            matches!(err, ConfigError::BadType { field, .. } if field == "report_interval_s")
        );
    }

    #[test]
    fn out_of_range_values_are_errors() {
        let err = Config::parse(
            r#"
            [transport.mqtt]
            port = 70000
            "#,
        )
        .expect_err("port above u16");
        assert!(matches!(err, ConfigError::BadValue { field, .. } if field == "port"));

        let err = Config::parse(
            r#"
            [sensors.bme280]
            i2c_address = 0xff
            "#,
        )
        .expect_err("i2c address above 7 bits");
        assert!(
            matches!(err, ConfigError::BadValue { field, .. } if field == "i2c_address")
        );

        let err = Config::parse(
            r#"
            [decibel]
            overload_threshold_db = -5.0
            "#,
        )
        .expect_err("negative overload threshold");
        assert!(
            matches!(err, ConfigError::BadValue { field, .. } if field == "overload_threshold_db")
        );
    }

    #[test]
    fn transport_active_rejects_unknown_kinds() {
        let err = Config::parse(
            r#"
            [transport]
            active = "carrier-pigeon"
            "#,
        )
        .expect_err("unknown transport kind");
        assert!(matches!(err, ConfigError::BadValue { field, .. } if field == "active"));
    }

    #[test]
    fn sensor_id_must_be_uuid_shaped() {
        let err = Config::parse(
            r#"
            [sensor]
            id = "not-a-uuid"
            "#,
        )
        .expect_err("malformed UUID");
        assert!(matches!(err, ConfigError::BadValue { field, .. } if field == "id"));
        assert!(is_uuid_like("0b1ac1e7-38f8-4f3e-9d0a-8b6a2c4e5f70"));
        assert!(!is_uuid_like("0b1ac1e738f84f3e9d0a8b6a2c4e5f70"));
    }

    #[test]
    fn to_toml_round_trips_the_committed_document() {
        let Parsed { config, .. } = committed();
        let text = config.to_toml();
        let reparsed = Config::parse(&text).expect("serialized config reparses");
        assert_eq!(reparsed.config, config, "round-trip must preserve values");
        assert!(
            reparsed.warnings.is_empty(),
            "serialized config is canonical: no warnings"
        );
    }

    #[test]
    fn serialized_output_passes_the_secret_tripwire() {
        let Parsed { config, .. } = committed();
        let text = config.to_toml();
        assert!(!serialized_output_contains_secret_marker(&text));
        // And the tripwire itself knows what it is looking for.
        assert!(serialized_output_contains_secret_marker(
            "wifi_password = \"x\""
        ));
        assert!(serialized_output_contains_secret_marker("the apikey value"));
    }

    #[test]
    // The assertion needs one transport on and the other off — the default
    // world (http on, mqtt off). In the all-off anchor both are false, and
    // in single-leaf builds the other transport may be on instead.
    #[cfg(all(not(feature = "transport-mqtt"), feature = "transport-http"))]
    fn uncompiled_transport_is_detected() {
        // In the default world mqtt is off: selecting it is a config edit
        // that must be caught at boot.
        assert!(Config::is_compiled(TransportKind::Http));
        assert!(!Config::is_compiled(TransportKind::Mqtt));
    }

    #[test]
    #[cfg(feature = "transport-mqtt")]
    fn compiled_transport_is_detected() {
        assert!(Config::is_compiled(TransportKind::Mqtt));
        assert_eq!(TransportKind::Mqtt.as_str(), "mqtt");
    }
}
