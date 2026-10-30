//! Runtime configuration — committed defaults, and the layers above them.
//!
//! Spec §Configuration: one canonical TOML file is committed at
//! `config/default.toml`, compiled into the binary with [`include_str`] as
//! the fleet defaults; NVS overrides are read on top at boot so a device is
//! tuned per-unit without a rebuild. Precedence is one rule, tested on the
//! host:
//!
//! ```text
//! NVS override > config/default.toml > built-in constant ([`Config::builtin`])
//! ```
//!
//! All three layers speak the same shape: built-ins are a full [`Config`],
//! the file and NVS layers are a [`PartialConfig`] applied over whatever is
//! beneath them. Every applied value passes semantic validation; a value
//! that fails (a zero interval, a `sensor_id` that is not a UUID) is
//! ignored and reported as a [`ConfigWarning`] — the device still boots
//! with sane defaults. Unknown keys are likewise tolerated: they land in
//! the warning list and change nothing, so a config file can carry
//! forward-compatible keys for a newer firmware.
//!
//! Secrets (Wi-Fi credentials, API key, client certificates) are not part
//! of this schema at all — they exist only in NVS, read at use time through
//! the [`crate::nvs::SecretStore`] seam, and are never serialized (the
//! serializer carries a debug assertion to that effect).
//!
//! Grammar: a deliberate TOML subset — `[table]` and `[table.sub]` headers,
//! bare keys, basic strings, decimal integers, floats, booleans, and `#`
//! comments. Arrays, inline tables, multi-line strings, and non-decimal
//! integers are rejected with [`TomlError`], not silently misread.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::ops::RangeInclusive;

use crate::measurement::is_uuid;

/// The committed defaults, compiled into the binary (spec §Configuration).
pub const DEFAULT_TOML: &str = include_str!("../config/default.toml");

/// Key-name fragments that must never appear in serialized config. The
/// schema has no secret fields by construction; this list is the belt to
/// that suspenders ([`Config::to_toml`] debug-asserts against it, and a
/// host test scans the committed `default.toml` with it).
pub const FORBIDDEN_SECRET_NAMES: &[&str] = &[
    "api_key",
    "apikey",
    "password",
    "passwd",
    "psk",
    "passphrase",
    "secret",
    "token",
    "cert",
    "credentials",
];

/// Every config key path, in the canonical order [`Config::to_toml`] writes
/// them. The committed `default.toml` must cover exactly this list — a host
/// test compares the file layer's [`PartialConfig::bound_keys`] against
/// this length, so a field added without documenting its default fails CI.
pub const TUNABLE_PATHS: &[&str] = &[
    "device.sensor_id",
    "transport.active",
    "transport.http.endpoint",
    "transport.http.timeout_ms",
    "transport.http.verify_tls",
    "transport.mqtt.host",
    "transport.mqtt.port",
    "transport.mqtt.keepalive_secs",
    "sampling.interval_ms",
    "sensors.decibel.interval_ms",
    "sensors.decibel.window_ms",
    "sensors.decibel.calibration_offset_db",
    "sensors.decibel.overload_pcm",
    "sensors.pax.interval_ms",
    "sensors.pax.pir_pin",
    "sensors.bme280.interval_ms",
    "sensors.sht4x.interval_ms",
    "sensors.scd4x.interval_ms",
    "sensors.pms5003.interval_ms",
    "sensors.bh1750.interval_ms",
    "publish.interval_ms",
    "publish.backoff_initial_ms",
    "publish.backoff_max_ms",
    "publish.offline_buffer",
    "status.heartbeat_ms",
];

/// Why a config layer was (partly) ignored.
///
/// Tolerant by design: none of these abort a boot — the layer's good keys
/// still apply, and the device keeps the layer-below value for the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigWarning {
    /// A key this firmware does not know (dotted path). Forward-compatible
    /// configs rely on this being a warning, not an error.
    UnknownKey(String),
    /// A known key whose value could not be used, with the reason.
    BadValue { key: String, reason: String },
    /// A whole layer was dropped — its source is not parseable at all.
    LayerSyntax(String),
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(key) => write!(f, "unknown config key `{key}` — ignored"),
            Self::BadValue { key, reason } => write!(
                f,
                "config key `{key}` unusable: {reason} — kept previous value"
            ),
            Self::LayerSyntax(msg) => write!(f, "config layer dropped: {msg}"),
        }
    }
}

/// Which compiled transport the device publishes through.
///
/// Deliberately closed: a new transport is a firmware change (it needs its
/// cargo flag and driver), never a config value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportKind {
    /// HTTP JSON ingest — the working server path.
    Http,
    /// MQTT v2 protobuf per `api/mqtt`.
    Mqtt,
}

impl TransportKind {
    /// The vocabulary accepted in `transport.active` (and in NVS).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "http" => Some(Self::Http),
            "mqtt" => Some(Self::Mqtt),
            _ => None,
        }
    }

    /// Canonical spelling, as serialized back to TOML.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Mqtt => "mqtt",
        }
    }
}

/// Identifies a compiled sensor when asking for its measurement cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorKind {
    Decibel,
    Pax,
    Bme280,
    Sht4x,
    Scd4x,
    Pms5003,
    Bh1750,
}

/// `[device]` — identity. The real per-device UUID arrives through NVS at
/// provisioning; the committed default is the nil-UUID factory placeholder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceConfig {
    /// Wire-contract `sensor_id`: a UUID, validated at every layer.
    pub sensor_id: String,
}

/// `[transport.http]` — the HTTP JSON ingest client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpTransportConfig {
    /// Ingest origin; the transport appends `/v1/sensors/<type>` (Caddy
    /// strips `/v1` for the Go service).
    pub endpoint: String,
    /// Per-request timeout for one measurement POST.
    pub timeout_ms: u32,
    /// Verify the server certificate chain (disable only on a bench).
    pub verify_tls: bool,
}

/// `[transport.mqtt]` — the MQTT v2 protobuf client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MqttTransportConfig {
    /// Broker host (`mqtt.clubins.de` in production).
    pub host: String,
    /// Broker port; 8883 is mQTTS.
    pub port: u16,
    /// MQTT keep-alive interval in seconds.
    pub keepalive_secs: u16,
}

/// `[transport]` — which compiled transport runs, plus its tuning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportConfig {
    /// The active transport; selecting one that was not compiled in fails
    /// at boot (a config edit tunes, never enables, a capability).
    pub active: TransportKind,
    pub http: HttpTransportConfig,
    pub mqtt: MqttTransportConfig,
}

/// `[sampling]` — the default measurement cadence for sensors that do not
/// set their own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplingConfig {
    /// Default cadence in milliseconds.
    pub interval_ms: u32,
}

/// `[sensors.decibel]` — the PDM-microphone front-end (DSP per decibel.md).
#[derive(Clone, Debug, PartialEq)]
pub struct DecibelConfig {
    /// Measurement cadence; `None` falls back to `[sampling]`.
    pub interval_ms: Option<u32>,
    /// Aggregation window for LAeq/LCeq and the Fast-detector extrema.
    pub window_ms: u32,
    /// Per-unit calibration offset in dB, added after level conversion;
    /// NVS-overridable per unit (decibel.md §Calibration).
    pub calibration_offset_db: f32,
    /// Raw-PCM clipping threshold; `i16::MAX` until measured on hardware
    /// (decibel.md §Overload). NVS-overridable.
    pub overload_pcm: u16,
}

/// `[sensors.pax]` — the PIR occupancy front-end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaxConfig {
    /// Measurement cadence; `None` falls back to `[sampling]`.
    pub interval_ms: Option<u32>,
    /// GPIO pin of the PIR front-end ("pin from config", spec §Sensors).
    pub pir_pin: u8,
}

/// A sensor section that only tunes cadence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SensorTiming {
    /// Measurement cadence; `None` falls back to `[sampling]`.
    pub interval_ms: Option<u32>,
}

/// `[sensors.*]` — per-sensor tuning, one section per compiled sensor.
#[derive(Clone, Debug, PartialEq)]
pub struct SensorTuning {
    pub decibel: DecibelConfig,
    pub pax: PaxConfig,
    pub bme280: SensorTiming,
    pub sht4x: SensorTiming,
    pub scd4x: SensorTiming,
    pub pms5003: SensorTiming,
    pub bh1750: SensorTiming,
}

/// `[publish]` — outbound cadence and offline resilience.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishConfig {
    /// Cadence for flushing measurements to the active transport.
    pub interval_ms: u32,
    /// Backoff on publish failures: start here and double up to the max.
    pub backoff_initial_ms: u32,
    /// Backoff ceiling.
    pub backoff_max_ms: u32,
    /// Bounded offline buffer (measurements held while the link is down).
    pub offline_buffer: u32,
}

/// `[status]` — device status reporting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusConfig {
    /// Heartbeat cadence while online (retained on MQTT connect).
    pub heartbeat_ms: u32,
}

/// The fully-resolved runtime configuration: built-ins with the
/// `default.toml` and NVS layers applied. Produced by [`Config::builtin`]
/// plus [`PartialConfig::apply`]; the boot seam that chains all three
/// layers is [`crate::nvs::load_config`].
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub device: DeviceConfig,
    pub transport: TransportConfig,
    pub sampling: SamplingConfig,
    pub sensors: SensorTuning,
    pub publish: PublishConfig,
    pub status: StatusConfig,
}

impl Config {
    /// The compiled-in last-resort layer of the precedence rule: the same
    /// values as the committed `default.toml` (a host test pins the two
    /// together), kept separately so a dropped or corrupt file layer still
    /// boots sane defaults.
    pub fn builtin() -> Self {
        Self {
            device: DeviceConfig {
                sensor_id: String::from("00000000-0000-0000-0000-000000000000"),
            },
            transport: TransportConfig {
                active: TransportKind::Http,
                http: HttpTransportConfig {
                    endpoint: String::from("https://ingest.clubins.de"),
                    timeout_ms: 5000,
                    verify_tls: true,
                },
                mqtt: MqttTransportConfig {
                    host: String::from("mqtt.clubins.de"),
                    port: 8883,
                    keepalive_secs: 60,
                },
            },
            sampling: SamplingConfig { interval_ms: 1000 },
            sensors: SensorTuning {
                decibel: DecibelConfig {
                    interval_ms: Some(1000),
                    window_ms: 60_000,
                    calibration_offset_db: 0.0,
                    overload_pcm: i16::MAX as u16,
                },
                pax: PaxConfig {
                    interval_ms: Some(1000),
                    pir_pin: 4,
                },
                bme280: SensorTiming {
                    interval_ms: Some(10_000),
                },
                sht4x: SensorTiming {
                    interval_ms: Some(10_000),
                },
                scd4x: SensorTiming {
                    interval_ms: Some(10_000),
                },
                pms5003: SensorTiming {
                    interval_ms: Some(30_000),
                },
                bh1750: SensorTiming {
                    interval_ms: Some(10_000),
                },
            },
            publish: PublishConfig {
                interval_ms: 15_000,
                backoff_initial_ms: 500,
                backoff_max_ms: 30_000,
                offline_buffer: 64,
            },
            status: StatusConfig {
                heartbeat_ms: 30_000,
            },
        }
    }

    /// Measurement cadence for one sensor: its own `[sensors.<name>]`
    /// cadence when set, otherwise the `[sampling]` default.
    pub fn sample_interval_ms(&self, kind: SensorKind) -> u32 {
        let specific = match kind {
            SensorKind::Decibel => self.sensors.decibel.interval_ms,
            SensorKind::Pax => self.sensors.pax.interval_ms,
            SensorKind::Bme280 => self.sensors.bme280.interval_ms,
            SensorKind::Sht4x => self.sensors.sht4x.interval_ms,
            SensorKind::Scd4x => self.sensors.scd4x.interval_ms,
            SensorKind::Pms5003 => self.sensors.pms5003.interval_ms,
            SensorKind::Bh1750 => self.sensors.bh1750.interval_ms,
        };
        specific.unwrap_or(self.sampling.interval_ms)
    }

    /// Serialize the full configuration back to TOML, sections and keys in
    /// the canonical order of `config/default.toml`. Used by the host
    /// round-trip test, and on the device to echo effective config into
    /// status reports — hence the debug assertion that nothing secret-like
    /// can leak into the output.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        put_table(&mut out, "device");
        put_str(&mut out, "sensor_id", &self.device.sensor_id);
        let t = &self.transport;
        put_table(&mut out, "transport");
        put_str(&mut out, "active", t.active.as_str());
        put_table(&mut out, "transport.http");
        put_str(&mut out, "endpoint", &t.http.endpoint);
        put_num(&mut out, "timeout_ms", t.http.timeout_ms);
        put_num(&mut out, "verify_tls", t.http.verify_tls);
        put_table(&mut out, "transport.mqtt");
        put_str(&mut out, "host", &t.mqtt.host);
        put_num(&mut out, "port", t.mqtt.port);
        put_num(&mut out, "keepalive_secs", t.mqtt.keepalive_secs);
        put_table(&mut out, "sampling");
        put_num(&mut out, "interval_ms", self.sampling.interval_ms);
        let s = &self.sensors;
        put_table(&mut out, "sensors.decibel");
        put_opt_num(&mut out, "interval_ms", s.decibel.interval_ms);
        put_num(&mut out, "window_ms", s.decibel.window_ms);
        put_num(
            &mut out,
            "calibration_offset_db",
            s.decibel.calibration_offset_db,
        );
        put_num(&mut out, "overload_pcm", s.decibel.overload_pcm);
        put_table(&mut out, "sensors.pax");
        put_opt_num(&mut out, "interval_ms", s.pax.interval_ms);
        put_num(&mut out, "pir_pin", s.pax.pir_pin);
        for (name, timing) in [
            ("sensors.bme280", &s.bme280),
            ("sensors.sht4x", &s.sht4x),
            ("sensors.scd4x", &s.scd4x),
            ("sensors.pms5003", &s.pms5003),
            ("sensors.bh1750", &s.bh1750),
        ] {
            put_table(&mut out, name);
            put_opt_num(&mut out, "interval_ms", timing.interval_ms);
        }
        put_table(&mut out, "publish");
        put_num(&mut out, "interval_ms", self.publish.interval_ms);
        put_num(
            &mut out,
            "backoff_initial_ms",
            self.publish.backoff_initial_ms,
        );
        put_num(&mut out, "backoff_max_ms", self.publish.backoff_max_ms);
        put_num(&mut out, "offline_buffer", self.publish.offline_buffer);
        put_table(&mut out, "status");
        put_num(&mut out, "heartbeat_ms", self.status.heartbeat_ms);
        debug_assert_no_secrets(&out);
        out
    }
}

/// One layer of overrides over a [`Config`] — the shape both the
/// `default.toml` binder and the NVS layer produce. Every field is `None`
/// unless that layer set it; [`PartialConfig::apply`] overwrites only what
/// is present and valid.
///
/// A layer carries the diagnostics from its own binding
/// ([`PartialConfig::warnings`]) and re-emits them when applied, so a
/// tolerance problem reported at parse time survives into the warn list
/// produced by the layered chain.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialConfig {
    pub device: PartialDevice,
    pub transport: PartialTransport,
    pub sampling: PartialSampling,
    pub sensors: PartialSensors,
    pub publish: PartialPublish,
    pub status: PartialStatus,
    /// What binding this layer had to ignore (unknown keys, bad values,
    /// a syntax error that dropped the whole layer).
    pub warnings: Vec<ConfigWarning>,
}

/// Layer overrides for `[device]` — every field absent unless set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialDevice {
    pub sensor_id: Option<String>,
}

/// Layer overrides for `[transport]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialTransport {
    pub active: Option<TransportKind>,
    pub http: PartialHttpTransport,
    pub mqtt: PartialMqttTransport,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialHttpTransport {
    pub endpoint: Option<String>,
    pub timeout_ms: Option<u32>,
    pub verify_tls: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialMqttTransport {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub keepalive_secs: Option<u16>,
}

/// Layer overrides for `[sampling]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialSampling {
    pub interval_ms: Option<u32>,
}

/// Layer overrides for `[sensors.decibel]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialDecibel {
    pub interval_ms: Option<u32>,
    pub window_ms: Option<u32>,
    pub calibration_offset_db: Option<f32>,
    pub overload_pcm: Option<u16>,
}

/// Layer overrides for `[sensors.pax]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialPax {
    pub interval_ms: Option<u32>,
    pub pir_pin: Option<u8>,
}

/// Layer overrides for a cadence-only sensor section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartialSensorTiming {
    pub interval_ms: Option<u32>,
}

/// Layer overrides for `[sensors.*]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialSensors {
    pub decibel: PartialDecibel,
    pub pax: PartialPax,
    pub bme280: PartialSensorTiming,
    pub sht4x: PartialSensorTiming,
    pub scd4x: PartialSensorTiming,
    pub pms5003: PartialSensorTiming,
    pub bh1750: PartialSensorTiming,
}

/// Layer overrides for `[publish]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialPublish {
    pub interval_ms: Option<u32>,
    pub backoff_initial_ms: Option<u32>,
    pub backoff_max_ms: Option<u32>,
    pub offline_buffer: Option<u32>,
}

/// Layer overrides for `[status]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialStatus {
    pub heartbeat_ms: Option<u32>,
}

impl PartialConfig {
    /// How many tunables this layer actually sets. The committed
    /// `default.toml` must reach [`TUNABLE_PATHS`].len() — every documented
    /// default present, none duplicated.
    pub fn bound_keys(&self) -> usize {
        let d = &self.device;
        let t = &self.transport;
        let s = &self.sensors;
        [
            d.sensor_id.is_some(),
            t.active.is_some(),
            t.http.endpoint.is_some(),
            t.http.timeout_ms.is_some(),
            t.http.verify_tls.is_some(),
            t.mqtt.host.is_some(),
            t.mqtt.port.is_some(),
            t.mqtt.keepalive_secs.is_some(),
            self.sampling.interval_ms.is_some(),
            s.decibel.interval_ms.is_some(),
            s.decibel.window_ms.is_some(),
            s.decibel.calibration_offset_db.is_some(),
            s.decibel.overload_pcm.is_some(),
            s.pax.interval_ms.is_some(),
            s.pax.pir_pin.is_some(),
            s.bme280.interval_ms.is_some(),
            s.sht4x.interval_ms.is_some(),
            s.scd4x.interval_ms.is_some(),
            s.pms5003.interval_ms.is_some(),
            s.bh1750.interval_ms.is_some(),
            self.publish.interval_ms.is_some(),
            self.publish.backoff_initial_ms.is_some(),
            self.publish.backoff_max_ms.is_some(),
            self.publish.offline_buffer.is_some(),
            self.status.heartbeat_ms.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count()
    }

    /// Parse one TOML layer into overrides. A syntax error drops the whole
    /// layer, leaving only a [`ConfigWarning::LayerSyntax`] on its warn
    /// list — the device boots on the layers beneath it.
    pub fn from_toml(src: &str) -> Self {
        match parse_toml(src) {
            Ok(doc) => {
                let (partial, warnings) = Self::bind(&doc);
                Self {
                    warnings,
                    ..partial
                }
            }
            Err(e) => Self {
                warnings: vec![ConfigWarning::LayerSyntax(e.to_string())],
                ..Self::default()
            },
        }
    }

    /// Validate and apply this layer onto `cfg`. Bad values are warned and
    /// skipped; the device keeps the value from the layer beneath.
    pub fn apply(&self, cfg: &mut Config, warnings: &mut Vec<ConfigWarning>) {
        warnings.extend(self.warnings.iter().cloned());
        self.device.apply(&mut cfg.device, warnings);
        self.transport.apply(&mut cfg.transport, warnings);
        self.sampling.apply(&mut cfg.sampling, warnings);
        self.sensors.apply(&mut cfg.sensors, warnings);
        self.publish.apply(&mut cfg.publish, warnings);
        self.status.apply(&mut cfg.status, warnings);
    }

    /// Compose full layers: start from `builtin`, apply each in order.
    /// The boot path is `from_layers(builtin, &[&file_layer, &nvs_layer])`.
    pub fn from_layers(
        builtin: Config,
        layers: &[&Self],
    ) -> (Config, Vec<ConfigWarning>) {
        let mut cfg = builtin;
        let mut warnings = Vec::new();
        for layer in layers {
            layer.apply(&mut cfg, &mut warnings);
        }
        (cfg, warnings)
    }

    /// Bind a parsed TOML document into overrides. Unknown keys and type
    /// mismatches become warnings; only well-typed known keys bind.
    fn bind(doc: &TomlTable) -> (Self, Vec<ConfigWarning>) {
        let mut partial = Self::default();
        let mut warnings = Vec::new();
        for (key, value) in doc {
            match key.as_str() {
                "device" => match value {
                    TomlValue::Table(t) => bind_device(t, &mut partial, &mut warnings),
                    _ => push_bad_type(&mut warnings, "device", value),
                },
                "transport" => match value {
                    TomlValue::Table(t) => bind_transport(t, &mut partial, &mut warnings),
                    _ => push_bad_type(&mut warnings, "transport", value),
                },
                "sampling" => match value {
                    TomlValue::Table(t) => bind_sampling(t, &mut partial, &mut warnings),
                    _ => push_bad_type(&mut warnings, "sampling", value),
                },
                "sensors" => match value {
                    TomlValue::Table(t) => bind_sensors(t, &mut partial, &mut warnings),
                    _ => push_bad_type(&mut warnings, "sensors", value),
                },
                "publish" => match value {
                    TomlValue::Table(t) => bind_publish(t, &mut partial, &mut warnings),
                    _ => push_bad_type(&mut warnings, "publish", value),
                },
                "status" => match value {
                    TomlValue::Table(t) => bind_status(t, &mut partial, &mut warnings),
                    _ => push_bad_type(&mut warnings, "status", value),
                },
                other => warnings.push(ConfigWarning::UnknownKey(String::from(other))),
            }
        }
        (partial, warnings)
    }
}

fn bind_device(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    bind_str(
        t,
        "sensor_id",
        "device.sensor_id",
        &mut p.device.sensor_id,
        w,
    );
    warn_unknown_in("device", &["sensor_id"], t, w);
}

fn bind_transport(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    for (key, value) in t {
        match key.as_str() {
            "active" => match value {
                TomlValue::Str(s) => match TransportKind::parse(s) {
                    Some(kind) => p.transport.active = Some(kind),
                    None => w.push(bad_value(
                        "transport.active",
                        format!(
                            "unknown transport `{s}` (expected \"http\" or \"mqtt\")"
                        ),
                    )),
                },
                _ => push_bad_type(w, "transport.active", value),
            },
            "http" => match value {
                TomlValue::Table(sub) => bind_http(sub, p, w),
                _ => push_bad_type(w, "transport.http", value),
            },
            "mqtt" => match value {
                TomlValue::Table(sub) => bind_mqtt(sub, p, w),
                _ => push_bad_type(w, "transport.mqtt", value),
            },
            other => w.push(ConfigWarning::UnknownKey(format!("transport.{other}"))),
        }
    }
}

fn bind_http(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    let http = &mut p.transport.http;
    bind_str(
        t,
        "endpoint",
        "transport.http.endpoint",
        &mut http.endpoint,
        w,
    );
    bind_uint(
        t,
        "timeout_ms",
        "transport.http.timeout_ms",
        &mut http.timeout_ms,
        w,
    );
    bind_bool(
        t,
        "verify_tls",
        "transport.http.verify_tls",
        &mut http.verify_tls,
        w,
    );
    warn_unknown_in(
        "transport.http",
        &["endpoint", "timeout_ms", "verify_tls"],
        t,
        w,
    );
}

fn bind_mqtt(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    let mqtt = &mut p.transport.mqtt;
    bind_str(t, "host", "transport.mqtt.host", &mut mqtt.host, w);
    bind_uint(t, "port", "transport.mqtt.port", &mut mqtt.port, w);
    bind_uint(
        t,
        "keepalive_secs",
        "transport.mqtt.keepalive_secs",
        &mut mqtt.keepalive_secs,
        w,
    );
    warn_unknown_in("transport.mqtt", &["host", "port", "keepalive_secs"], t, w);
}

fn bind_sampling(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    bind_uint(
        t,
        "interval_ms",
        "sampling.interval_ms",
        &mut p.sampling.interval_ms,
        w,
    );
    warn_unknown_in("sampling", &["interval_ms"], t, w);
}

fn bind_sensors(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    for (key, value) in t {
        match key.as_str() {
            "decibel" => match value {
                TomlValue::Table(sub) => bind_decibel(sub, p, w),
                _ => push_bad_type(w, "sensors.decibel", value),
            },
            "pax" => match value {
                TomlValue::Table(sub) => bind_pax(sub, p, w),
                _ => push_bad_type(w, "sensors.pax", value),
            },
            "bme280" | "sht4x" | "scd4x" | "pms5003" | "bh1750" => match value {
                TomlValue::Table(sub) => {
                    let slot = match key.as_str() {
                        "bme280" => &mut p.sensors.bme280.interval_ms,
                        "sht4x" => &mut p.sensors.sht4x.interval_ms,
                        "scd4x" => &mut p.sensors.scd4x.interval_ms,
                        "pms5003" => &mut p.sensors.pms5003.interval_ms,
                        _ => &mut p.sensors.bh1750.interval_ms,
                    };
                    let path = format!("sensors.{key}.interval_ms");
                    bind_uint(sub, "interval_ms", &path, slot, w);
                    for sub_key in sub.keys() {
                        if sub_key != "interval_ms" {
                            w.push(ConfigWarning::UnknownKey(format!(
                                "sensors.{key}.{sub_key}"
                            )));
                        }
                    }
                }
                _ => push_bad_type(w, "sensors", value),
            },
            other => w.push(ConfigWarning::UnknownKey(format!("sensors.{other}"))),
        }
    }
}

fn bind_decibel(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    let d = &mut p.sensors.decibel;
    bind_uint(
        t,
        "interval_ms",
        "sensors.decibel.interval_ms",
        &mut d.interval_ms,
        w,
    );
    bind_uint(
        t,
        "window_ms",
        "sensors.decibel.window_ms",
        &mut d.window_ms,
        w,
    );
    bind_float(
        t,
        "calibration_offset_db",
        "sensors.decibel.calibration_offset_db",
        &mut d.calibration_offset_db,
        w,
    );
    bind_uint(
        t,
        "overload_pcm",
        "sensors.decibel.overload_pcm",
        &mut d.overload_pcm,
        w,
    );
    warn_unknown_in(
        "sensors.decibel",
        &[
            "interval_ms",
            "window_ms",
            "calibration_offset_db",
            "overload_pcm",
        ],
        t,
        w,
    );
}

fn bind_pax(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    let x = &mut p.sensors.pax;
    bind_uint(
        t,
        "interval_ms",
        "sensors.pax.interval_ms",
        &mut x.interval_ms,
        w,
    );
    bind_uint(t, "pir_pin", "sensors.pax.pir_pin", &mut x.pir_pin, w);
    warn_unknown_in("sensors.pax", &["interval_ms", "pir_pin"], t, w);
}

fn bind_publish(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    let out = &mut p.publish;
    bind_uint(
        t,
        "interval_ms",
        "publish.interval_ms",
        &mut out.interval_ms,
        w,
    );
    bind_uint(
        t,
        "backoff_initial_ms",
        "publish.backoff_initial_ms",
        &mut out.backoff_initial_ms,
        w,
    );
    bind_uint(
        t,
        "backoff_max_ms",
        "publish.backoff_max_ms",
        &mut out.backoff_max_ms,
        w,
    );
    bind_uint(
        t,
        "offline_buffer",
        "publish.offline_buffer",
        &mut out.offline_buffer,
        w,
    );
    warn_unknown_in(
        "publish",
        &[
            "interval_ms",
            "backoff_initial_ms",
            "backoff_max_ms",
            "offline_buffer",
        ],
        t,
        w,
    );
}

fn bind_status(t: &TomlTable, p: &mut PartialConfig, w: &mut Vec<ConfigWarning>) {
    bind_uint(
        t,
        "heartbeat_ms",
        "status.heartbeat_ms",
        &mut p.status.heartbeat_ms,
        w,
    );
    warn_unknown_in("status", &["heartbeat_ms"], t, w);
}

// --- typed slot binders: shape errors here, semantics at apply time ------

/// Tolerance guard: every section binder ends with this, so a key the
/// schema does not know still lands on the warn list (spec §Configuration)
/// instead of vanishing.
fn warn_unknown_in(
    section: &str,
    known: &[&str],
    t: &TomlTable,
    w: &mut Vec<ConfigWarning>,
) {
    for key in t.keys() {
        if !known.contains(&key.as_str()) {
            w.push(ConfigWarning::UnknownKey(format!("{section}.{key}")));
        }
    }
}

fn bind_str(
    t: &TomlTable,
    key: &str,
    path: &str,
    slot: &mut Option<String>,
    w: &mut Vec<ConfigWarning>,
) {
    if let Some(value) = t.get(key) {
        match value {
            TomlValue::Str(s) => *slot = Some(s.clone()),
            _ => push_bad_type(w, path, value),
        }
    }
}

fn bind_bool(
    t: &TomlTable,
    key: &str,
    path: &str,
    slot: &mut Option<bool>,
    w: &mut Vec<ConfigWarning>,
) {
    if let Some(value) = t.get(key) {
        match value {
            TomlValue::Bool(b) => *slot = Some(*b),
            _ => push_bad_type(w, path, value),
        }
    }
}

/// Unsigned integer field: the TOML value must be an integer that fits the
/// slot's width.
fn bind_uint<T: TryFrom<i64>>(
    t: &TomlTable,
    key: &str,
    path: &str,
    slot: &mut Option<T>,
    w: &mut Vec<ConfigWarning>,
) {
    if let Some(value) = t.get(key) {
        match value {
            TomlValue::Int(n) => match T::try_from(*n) {
                Ok(v) => *slot = Some(v),
                Err(_) => w.push(bad_value(
                    path,
                    format!("`{n}` does not fit this field's range"),
                )),
            },
            _ => push_bad_type(w, path, value),
        }
    }
}

/// Float field: accepts a TOML float or integer (`offset = 0` means `0.0`),
/// per the tolerance rule.
fn bind_float(
    t: &TomlTable,
    key: &str,
    path: &str,
    slot: &mut Option<f32>,
    w: &mut Vec<ConfigWarning>,
) {
    if let Some(value) = t.get(key) {
        match value {
            TomlValue::Float(x) => *slot = Some(*x as f32),
            TomlValue::Int(n) => *slot = Some(*n as f32),
            _ => push_bad_type(w, path, value),
        }
    }
}

fn push_bad_type(w: &mut Vec<ConfigWarning>, path: &str, value: &TomlValue) {
    let found = match value {
        TomlValue::Table(_) => "a table",
        TomlValue::Str(_) => "a string",
        TomlValue::Int(_) => "an integer",
        TomlValue::Float(_) => "a float",
        TomlValue::Bool(_) => "a boolean",
    };
    w.push(bad_value(
        path,
        format!("expected the field's type, found {found}"),
    ));
}

fn bad_value(key: &str, reason: impl Into<String>) -> ConfigWarning {
    ConfigWarning::BadValue {
        key: String::from(key),
        reason: reason.into(),
    }
}

// --- per-section semantic validation + application -----------------------

impl PartialDevice {
    fn apply(&self, cfg: &mut DeviceConfig, w: &mut Vec<ConfigWarning>) {
        if let Some(id) = &self.sensor_id {
            if is_uuid(id) {
                cfg.sensor_id = id.clone();
            } else {
                w.push(bad_value("device.sensor_id", "must be a UUID"));
            }
        }
    }
}

impl PartialTransport {
    fn apply(&self, cfg: &mut TransportConfig, w: &mut Vec<ConfigWarning>) {
        if let Some(active) = self.active {
            cfg.active = active;
        }
        let http = &self.http;
        if let Some(endpoint) = &http.endpoint {
            if endpoint.starts_with("https://") || endpoint.starts_with("http://") {
                cfg.http.endpoint = endpoint.clone();
            } else {
                w.push(bad_value(
                    "transport.http.endpoint",
                    "must be an http(s) URL",
                ));
            }
        }
        apply_range(
            http.timeout_ms,
            &mut cfg.http.timeout_ms,
            "transport.http.timeout_ms",
            1..=u32::MAX,
            w,
        );
        if let Some(verify) = http.verify_tls {
            cfg.http.verify_tls = verify;
        }
        let mqtt = &self.mqtt;
        if let Some(host) = &mqtt.host {
            if host.is_empty() {
                w.push(bad_value("transport.mqtt.host", "must not be empty"));
            } else {
                cfg.mqtt.host = host.clone();
            }
        }
        apply_range(
            mqtt.port,
            &mut cfg.mqtt.port,
            "transport.mqtt.port",
            1..=u16::MAX,
            w,
        );
        apply_range(
            mqtt.keepalive_secs,
            &mut cfg.mqtt.keepalive_secs,
            "transport.mqtt.keepalive_secs",
            1..=3600,
            w,
        );
    }
}

impl PartialSampling {
    fn apply(&self, cfg: &mut SamplingConfig, w: &mut Vec<ConfigWarning>) {
        apply_range(
            self.interval_ms,
            &mut cfg.interval_ms,
            "sampling.interval_ms",
            1..=u32::MAX,
            w,
        );
    }
}

impl PartialSensors {
    fn apply(&self, cfg: &mut SensorTuning, w: &mut Vec<ConfigWarning>) {
        let d = &self.decibel;
        apply_opt_range(
            d.interval_ms,
            &mut cfg.decibel.interval_ms,
            "sensors.decibel.interval_ms",
            1..=u32::MAX,
            w,
        );
        apply_range(
            d.window_ms,
            &mut cfg.decibel.window_ms,
            "sensors.decibel.window_ms",
            1_000..=600_000,
            w,
        );
        if let Some(offset) = d.calibration_offset_db {
            if offset.is_finite() && offset.abs() <= 40.0 {
                cfg.decibel.calibration_offset_db = offset;
            } else {
                w.push(bad_value(
                    "sensors.decibel.calibration_offset_db",
                    "must be finite and within ±40 dB",
                ));
            }
        }
        apply_range(
            d.overload_pcm,
            &mut cfg.decibel.overload_pcm,
            "sensors.decibel.overload_pcm",
            1..=32_768,
            w,
        );
        let x = &self.pax;
        apply_opt_range(
            x.interval_ms,
            &mut cfg.pax.interval_ms,
            "sensors.pax.interval_ms",
            1..=u32::MAX,
            w,
        );
        apply_range(
            x.pir_pin,
            &mut cfg.pax.pir_pin,
            "sensors.pax.pir_pin",
            0..=48,
            w,
        );
        apply_opt_range(
            self.bme280.interval_ms,
            &mut cfg.bme280.interval_ms,
            "sensors.bme280.interval_ms",
            1..=u32::MAX,
            w,
        );
        apply_opt_range(
            self.sht4x.interval_ms,
            &mut cfg.sht4x.interval_ms,
            "sensors.sht4x.interval_ms",
            1..=u32::MAX,
            w,
        );
        apply_opt_range(
            self.scd4x.interval_ms,
            &mut cfg.scd4x.interval_ms,
            "sensors.scd4x.interval_ms",
            1..=u32::MAX,
            w,
        );
        apply_opt_range(
            self.pms5003.interval_ms,
            &mut cfg.pms5003.interval_ms,
            "sensors.pms5003.interval_ms",
            1..=u32::MAX,
            w,
        );
        apply_opt_range(
            self.bh1750.interval_ms,
            &mut cfg.bh1750.interval_ms,
            "sensors.bh1750.interval_ms",
            1..=u32::MAX,
            w,
        );
    }
}

impl PartialPublish {
    fn apply(&self, cfg: &mut PublishConfig, w: &mut Vec<ConfigWarning>) {
        apply_range(
            self.interval_ms,
            &mut cfg.interval_ms,
            "publish.interval_ms",
            1..=u32::MAX,
            w,
        );
        // The backoff pair is checked together: a max below the initial
        // value would make the schedule oscillate.
        let initial = self.backoff_initial_ms.unwrap_or(cfg.backoff_initial_ms);
        let max = self.backoff_max_ms.unwrap_or(cfg.backoff_max_ms);
        if max < initial {
            w.push(bad_value(
                "publish.backoff_max_ms",
                "must be at least backoff_initial_ms",
            ));
        } else {
            apply_range(
                self.backoff_initial_ms,
                &mut cfg.backoff_initial_ms,
                "publish.backoff_initial_ms",
                1..=u32::MAX,
                w,
            );
            apply_range(
                self.backoff_max_ms,
                &mut cfg.backoff_max_ms,
                "publish.backoff_max_ms",
                1..=u32::MAX,
                w,
            );
        }
        apply_range(
            self.offline_buffer,
            &mut cfg.offline_buffer,
            "publish.offline_buffer",
            1..=1024,
            w,
        );
    }
}

impl PartialStatus {
    fn apply(&self, cfg: &mut StatusConfig, w: &mut Vec<ConfigWarning>) {
        apply_range(
            self.heartbeat_ms,
            &mut cfg.heartbeat_ms,
            "status.heartbeat_ms",
            1..=u32::MAX,
            w,
        );
    }
}

/// Apply a value that must sit in an inclusive range onto a resolved
/// field; out-of-range values warn and keep the layer-below value.
fn apply_range<T: Copy + PartialOrd>(
    slot: Option<T>,
    field: &mut T,
    key: &str,
    range: RangeInclusive<T>,
    w: &mut Vec<ConfigWarning>,
) {
    if let Some(value) = slot {
        if range.contains(&value) {
            *field = value;
        } else {
            w.push(bad_value(key, "value out of range for this field"));
        }
    }
}

/// Same, onto an optional per-sensor field (`None` = "use the `[sampling]`
/// default") — an out-of-range value warns and leaves the fallback intact.
fn apply_opt_range<T: Copy + PartialOrd>(
    slot: Option<T>,
    field: &mut Option<T>,
    key: &str,
    range: RangeInclusive<T>,
    w: &mut Vec<ConfigWarning>,
) {
    if let Some(value) = slot {
        if range.contains(&value) {
            *field = Some(value);
        } else {
            w.push(bad_value(key, "value out of range for this field"));
        }
    }
}

// --- the TOML subset parser ----------------------------------------------

/// A parsed TOML value. Tables nest; scalars bind into [`PartialConfig`].
#[derive(Clone, Debug, PartialEq)]
pub enum TomlValue {
    Table(BTreeMap<String, TomlValue>),
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// A parsed document: the top-level table of keys → values.
pub type TomlTable = BTreeMap<String, TomlValue>;

/// Why a TOML source was rejected. A whole-layer failure — the boot path
/// keeps the layers beneath it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TomlError {
    /// 1-based line of the offending input.
    pub line: usize,
    pub kind: TomlErrorKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TomlErrorKind {
    /// Malformed line (bad key, missing `=`, trailing garbage, …).
    Syntax(&'static str),
    /// The same key assigned twice in one table.
    DuplicateKey(String),
    /// The same `[table]` header opened twice.
    DuplicateTable(String),
    /// A deliberate omission of the subset: arrays, inline tables,
    /// multi-line strings, non-decimal integers.
    Unsupported(&'static str),
}

impl fmt::Display for TomlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "config line {}: ", self.line)?;
        match &self.kind {
            TomlErrorKind::Syntax(what) => write!(f, "syntax: {what}"),
            TomlErrorKind::DuplicateKey(key) => write!(f, "duplicate key `{key}`"),
            TomlErrorKind::DuplicateTable(name) => write!(f, "duplicate table `{name}`"),
            TomlErrorKind::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

impl core::error::Error for TomlError {}

/// Parse the firmware's TOML subset into a document. Two table levels are
/// used by the schema (`[transport.http]`, `[sensors.decibel]`); headers
/// may nest deeper but must be opened top-down.
pub fn parse_toml(src: &str) -> Result<TomlTable, TomlError> {
    let mut root = TomlTable::new();
    let mut current: Vec<String> = Vec::new();
    let mut opened_tables: Vec<String> = Vec::new();
    for (idx, raw_line) in src.lines().enumerate() {
        let line = idx + 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('[') {
            let path = parse_header(rest, line)?;
            open_table(&mut root, &path, &mut opened_tables, line)?;
            current = path;
            continue;
        }
        let (key, rest) = parse_key_value(trimmed, line)?;
        let value = parse_value(rest, line)?;
        let table = table_mut(&mut root, &current);
        if table.contains_key(&key) {
            return Err(TomlError {
                line,
                kind: TomlErrorKind::DuplicateKey(key),
            });
        }
        table.insert(key, value);
    }
    Ok(root)
}

/// `[a.b]` header: everything after the opening `[`, validated, dotted path
/// returned.
fn parse_header(rest: &str, line: usize) -> Result<Vec<String>, TomlError> {
    let err = |what| TomlError {
        line,
        kind: TomlErrorKind::Syntax(what),
    };
    let (inner, _) =
        split_comment(rest).ok_or_else(|| err("unterminated table header"))?;
    let inner = inner
        .strip_suffix(']')
        .ok_or_else(|| err("table header must end with `]`"))?;
    if inner.is_empty() {
        return Err(err("empty table name"));
    }
    let mut path = Vec::new();
    for part in inner.split('.') {
        let name = parse_bare_key(part.trim()).map_err(|_| err("invalid table name"))?;
        path.push(name);
    }
    Ok(path)
}

/// `key = value` on one line: returns the bare key and the value segment.
fn parse_key_value(line: &str, line_no: usize) -> Result<(String, &str), TomlError> {
    let err = |what| TomlError {
        line: line_no,
        kind: TomlErrorKind::Syntax(what),
    };
    let (key_part, rest) = line
        .split_once('=')
        .ok_or_else(|| err("expected `key = value`"))?;
    let key = parse_bare_key(key_part.trim()).map_err(|_| err("invalid key"))?;
    if rest.trim().is_empty() {
        return Err(err("missing value"));
    }
    Ok((key, rest))
}

/// Bare TOML keys: letters, digits, `-`, `_`. (Quoted keys are outside the
/// subset; the committed config never needs them.)
fn parse_bare_key(s: &str) -> Result<String, TomlError> {
    let valid = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(String::from(s))
    } else {
        Err(TomlError {
            line: 0,
            kind: TomlErrorKind::Syntax("invalid key"),
        })
    }
}

/// Open a `[table]` header: create the path top-down, rejecting a header
/// opened twice.
fn open_table(
    root: &mut TomlTable,
    path: &[String],
    opened: &mut Vec<String>,
    line: usize,
) -> Result<(), TomlError> {
    let joined = path.join(".");
    if opened.contains(&joined) {
        return Err(TomlError {
            line,
            kind: TomlErrorKind::DuplicateTable(joined),
        });
    }
    opened.push(joined);
    table_mut(root, path);
    Ok(())
}

/// Walk (creating as needed) the table at `path`. A scalar occupying a name
/// the path needs is overwritten — a lossy normalization for a case the
/// duplicate checks make near-impossible in a well-formed file.
fn table_mut<'a>(root: &'a mut TomlTable, path: &[String]) -> &'a mut TomlTable {
    let mut table = root;
    for name in path {
        let entry = table
            .entry(name.clone())
            .or_insert_with(|| TomlValue::Table(BTreeMap::new()));
        if !matches!(entry, TomlValue::Table(_)) {
            *entry = TomlValue::Table(BTreeMap::new());
        }
        table = match entry {
            TomlValue::Table(inner) => inner,
            _ => unreachable!("normalized above"),
        };
    }
    table
}

/// Strip a trailing `# comment` from a header/key segment. Headers and bare
/// keys never contain `#` in this subset, so the first `#` ends the line.
fn split_comment(rest: &str) -> Option<(&str, &str)> {
    match rest.find('#') {
        Some(i) => Some((&rest[..i], &rest[i..])),
        None => Some((rest, "")),
    }
}

/// Parse the value segment of a `key = value` line into a [`TomlValue`].
fn parse_value(segment: &str, line: usize) -> Result<TomlValue, TomlError> {
    let err_syntax = |what| TomlError {
        line,
        kind: TomlErrorKind::Syntax(what),
    };
    let err_unsupported = |what| TomlError {
        line,
        kind: TomlErrorKind::Unsupported(what),
    };
    let trimmed = segment.trim_start();
    if trimmed.is_empty() {
        return Err(err_syntax("missing value"));
    }
    match trimmed.as_bytes()[0] {
        b'"' => {
            if trimmed.starts_with("\"\"\"") {
                return Err(err_unsupported("multi-line strings"));
            }
            let (decoded, rest) = scan_basic_string(trimmed, line)?;
            expect_line_end(rest, line)?;
            Ok(TomlValue::Str(decoded))
        }
        b'\'' => Err(err_unsupported("literal strings")),
        b'[' => Err(err_unsupported("arrays")),
        b'{' => Err(err_unsupported("inline tables")),
        _ => {
            let token_end = trimmed
                .find(|c: char| c.is_whitespace() || c == '#')
                .unwrap_or(trimmed.len());
            let token = &trimmed[..token_end];
            let rest = &trimmed[token_end..];
            let value = parse_scalar(token, line)?;
            expect_line_end(rest, line)?;
            Ok(value)
        }
    }
}

/// A scalar token: boolean, decimal integer (underscores allowed), or float.
fn parse_scalar(token: &str, line: usize) -> Result<TomlValue, TomlError> {
    match token {
        "true" => return Ok(TomlValue::Bool(true)),
        "false" => return Ok(TomlValue::Bool(false)),
        _ => {}
    }
    let cleaned = token.replace('_', "");
    if cleaned.starts_with("0x") || cleaned.starts_with("0o") || cleaned.starts_with("0b")
    {
        return Err(TomlError {
            line,
            kind: TomlErrorKind::Unsupported("non-decimal integers"),
        });
    }
    if let Ok(n) = cleaned.parse::<i64>() {
        return Ok(TomlValue::Int(n));
    }
    // TOML floats need a fraction or exponent; str::parse enforces that for
    // us (plain integers already returned above).
    if let Ok(x) = cleaned.parse::<f64>() {
        if x.is_finite() {
            return Ok(TomlValue::Float(x));
        }
        return Err(TomlError {
            line,
            kind: TomlErrorKind::Syntax("float out of range"),
        });
    }
    Err(TomlError {
        line,
        kind: TomlErrorKind::Syntax("unrecognized value"),
    })
}

/// Scan a basic string starting at `"`, honouring the escapes the subset
/// supports (`\" \\ \n \t \r \b \f`). Returns the decoded content and the
/// text after the closing quote.
fn scan_basic_string(src: &str, line: usize) -> Result<(String, &str), TomlError> {
    let mut out = String::new();
    let mut chars = src.char_indices().skip(1); // skip the opening quote
    while let Some((i, ch)) = chars.next() {
        match ch {
            '"' => {
                let rest = &src[i + ch.len_utf8()..];
                return Ok((out, rest));
            }
            '\\' => match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, 'r')) => out.push('\r'),
                Some((_, 'b')) => out.push('\u{8}'),
                Some((_, 'f')) => out.push('\u{c}'),
                Some((_, '"')) => out.push('"'),
                Some((_, '\\')) => out.push('\\'),
                Some((_, _)) => {
                    return Err(TomlError {
                        line,
                        kind: TomlErrorKind::Unsupported("escape sequence"),
                    });
                }
                None => {
                    return Err(TomlError {
                        line,
                        kind: TomlErrorKind::Syntax("unterminated escape"),
                    });
                }
            },
            _ => out.push(ch),
        }
    }
    Err(TomlError {
        line,
        kind: TomlErrorKind::Syntax("unterminated string"),
    })
}

/// After a value, only whitespace and/or a comment may follow.
fn expect_line_end(rest: &str, line: usize) -> Result<(), TomlError> {
    let trimmed = rest.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        Ok(())
    } else {
        Err(TomlError {
            line,
            kind: TomlErrorKind::Syntax("unexpected characters after value"),
        })
    }
}

// --- serializer helpers ---------------------------------------------------

fn put_table(out: &mut String, name: &str) {
    out.push('\n');
    out.push('[');
    out.push_str(name);
    out.push_str("]\n");
}

fn put_str(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = \"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out.push_str("\"\n");
}

/// Numbers and booleans — anything `Display`-able as valid TOML. Floats
/// print in their shortest round-trip form.
fn put_num<T: fmt::Display>(out: &mut String, key: &str, value: T) {
    out.push_str(key);
    out.push_str(" = ");
    // fmt::Write for String is infallible (it allocates).
    let _ = core::fmt::Write::write_fmt(out, format_args!("{value}"));
    out.push('\n');
}

fn put_opt_num<T: fmt::Display>(out: &mut String, key: &str, value: Option<T>) {
    if let Some(v) = value {
        put_num(out, key, v);
    }
}

/// Every dotted key path in a parsed document — the seam the secret
/// checks walk: key NAMES, not comment prose (a comment mentioning
/// "secrets" is documentation, a key named `api_key` is a leak).
pub fn config_key_paths(table: &TomlTable) -> Vec<String> {
    fn walk(prefix: &str, table: &TomlTable, out: &mut Vec<String>) {
        for (key, value) in table {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            match value {
                TomlValue::Table(nested) => {
                    out.push(path.clone());
                    walk(&path, nested, out);
                }
                _ => out.push(path),
            }
        }
    }
    let mut out = Vec::new();
    walk("", table, &mut out);
    out
}

/// The serializer's secret guard: in debug builds, refuse to hand out
/// serialized config whose KEY names anything secret-like. Runs on every
/// [`Config::to_toml`] call; a host test pins the committed
/// `default.toml` against the same list.
fn debug_assert_no_secrets(serialized: &str) {
    #[cfg(debug_assertions)]
    {
        match parse_toml(serialized) {
            Ok(doc) => {
                for path in config_key_paths(&doc) {
                    let lower = path.to_ascii_lowercase();
                    for name in FORBIDDEN_SECRET_NAMES {
                        debug_assert!(
                            !lower.split('.').any(|seg| seg.contains(name)),
                            "serialized config must never contain secret field `{name}` (key `{path}`)",
                        );
                    }
                }
            }
            Err(e) => panic!("Config::to_toml must produce parseable TOML: {e}"),
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = serialized;
}
