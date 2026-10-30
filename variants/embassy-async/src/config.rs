//! Three-tier runtime configuration — built-in constants, the committed
//! `config/default.toml`, and NVS overrides, merged in that order at boot.
//!
//! Precedence is one rule, host-tested (spec §Configuration): an NVS value
//! beats the `config/default.toml` value, which beats the built-in constant.
//! [`Config::from_tiers`] owns the merge. `config/default.toml` is embedded
//! with `include_str!` so a fresh flash boots with the committed defaults,
//! and NVS overrides tune a device — identity, calibration, cadences,
//! endpoints, active transport — without a rebuild. Enabling a capability
//! that was not compiled in is deliberately *not* possible from config; that
//! is the cargo-flag half of the framework.
//!
//! Secrets — Wi-Fi credentials, API key, client certificate and key — live
//! only in NVS. They are read exclusively through [`Secrets::from_overrides`]
//! and are not fields of [`Config`], so they cannot appear in the serialized
//! configuration ([`Config::to_toml`]) or its `Debug` output; the host tests
//! pin that with canary values, including a `[secret]` section smuggled into
//! a TOML file.
//!
//! The override source is the trait [`OverrideSource`]. The device
//! personality implements it over NVS at boot
//! (`Config::from_tiers(config::DEFAULT_TOML, &nvs)`); on the host, tests
//! inject [`StaticOverrides`] tables — which is why precedence needs no
//! hardware to verify.
//!
//! TOML subset parsed here: `[section]` / `[section.sub]` headers, `key =
//! value` with quoted strings, decimal and `0x` hex integers, decimal
//! floats, and full-line or inline comments. Everything else is tolerated:
//! unknown keys and sections, unparseable or wrong-typed values, and
//! oversized strings all leave the field at its lower-tier value — boot
//! never fails on config input, from any tier.

use core::fmt::{self, Write as _};

use heapless::{String as HeapString, Vec as HeapVec};

/// The committed defaults, compiled into the binary (file tier).
pub const DEFAULT_TOML: &str = include_str!("../config/default.toml");

/// Every non-secret config key, as a dotted path into the TOML structure.
/// The override tier is pulled per key from this list, so a key missing here
/// is a key NVS cannot tune.
pub const CONFIG_KEYS: [&str; 12] = [
    "sensor.id",
    "sampling.decibel_window_ms",
    "sampling.report_interval_s",
    "sampling.offline_buffer",
    "sensors.bme280.i2c_address",
    "sensors.pir.gpio_pin",
    "transport.active",
    "transport.http_endpoint",
    "transport.mqtt.broker",
    "transport.mqtt.port",
    "decibel.overload_threshold_db",
    "decibel.calibration_offset_db",
];

/// The NVS-only secret keys. [`Secrets::from_overrides`] reads these and
/// nothing else; no other code path may.
pub const SECRET_KEYS: [&str; 5] = [
    "secret.wifi_ssid",
    "secret.wifi_password",
    "secret.api_key",
    "secret.client_cert",
    "secret.client_key",
];

// ---------------------------------------------------------------------------
// Built-in constants — the bottom tier of the precedence rule
// ---------------------------------------------------------------------------

/// Placeholder sensor UUID (`[sensor] id`). The real per-device identity is
/// provisioned into NVS; this placeholder is what a fresh, unprovisioned
/// flash reports.
pub const SENSOR_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Measurements buffered between sensor tasks and the transport task.
/// Sending blocks (backpressure) when the transport falls this far behind;
/// the bound keeps RAM use flat instead of hiding slowdowns in a growing
/// heap. An architecture constant, not a tunable: it sizes static channels.
pub const MEASUREMENT_QUEUE_DEPTH: usize = 16;

/// Status events buffered between ops tasks and the transport task. Same
/// rationale as [`MEASUREMENT_QUEUE_DEPTH`].
pub const STATUS_QUEUE_DEPTH: usize = 8;

/// Built-in cadence of the status heartbeat, in ms
/// (`sampling.report_interval_s`).
pub const REPORT_INTERVAL_MS: u64 = 60_000;

/// Built-in length of one decibel measurement window, in ms
/// (`sampling.decibel_window_ms`). The 125 ms Fast detector runs inside it.
pub const DECIBEL_WINDOW_MS: u32 = 1_000;

/// Built-in capacity for measurements held while disconnected
/// (`sampling.offline_buffer`).
pub const OFFLINE_BUFFER: u32 = 64;

/// Poll cadence for sensor task skeletons whose drivers have not landed yet.
pub const SENSOR_POLL_INTERVAL_MS: u64 = 1_000;

/// Upper bound of the sensor task error backoff (`sensor::SensorTask`):
/// a permanently faulted sensor retries a few times a minute instead of
/// spinning on its bus. An architecture constant, not a tunable — it shapes
/// the fault behaviour of every sensor task at once.
pub const MAX_SENSOR_BACKOFF_MS: u64 = 30_000;

/// Built-in transport selection (`transport.active`). Values: `"http" |
/// "mqtt"`.
pub const ACTIVE_TRANSPORT: &str = "http";

/// Built-in ingest endpoint (`transport.http_endpoint`).
pub const HTTP_ENDPOINT: &str = "https://ingest.clubins.de/v1/sensors";

/// Built-in MQTT broker (`transport.mqtt.broker`).
pub const MQTT_BROKER: &str = "mqtt.clubins.de";

/// Built-in MQTT TLS listener port (`transport.mqtt.port`); plain 1883 only
/// without mTLS.
pub const MQTT_PORT: u16 = 8883;

/// Built-in BME280 I2C address (`sensors.bme280.i2c_address`).
pub const BME280_I2C_ADDRESS: u8 = 0x76;

/// Built-in PIR GPIO pin (`sensors.pir.gpio_pin`).
pub const PIR_GPIO_PIN: u8 = 4;

/// Built-in top of the mic's measurement range
/// (`decibel.overload_threshold_db`).
pub const OVERLOAD_THRESHOLD_DB: f32 = 120.0;

/// Built-in per-unit calibration (`decibel.calibration_offset_db`); the real
/// per-unit value arrives from NVS at runtime.
pub const CALIBRATION_OFFSET_DB: f32 = 0.0;

/// Upper bound on concurrently planned tasks (see `features::planned_tasks`).
pub const MAX_TASKS: usize = 16;

// ---------------------------------------------------------------------------
// TOML subset — the file tier's format
// ---------------------------------------------------------------------------

/// Longest dotted key the parser keeps (section + key).
const KEY_MAX: usize = 64;

/// Longest key/value table parsed from one TOML text; further entries are
/// dropped. The committed file holds 12 keys — headroom for local test
/// fixtures, far below any real file.
const TABLE_MAX: usize = 64;

/// Size of the serialized-config buffer ([`Config::to_toml`]). The
/// round-trip test pins adequacy.
const CONFIG_TOML_MAX: usize = 1024;

/// One parsed TOML value: the text it came from plus its typed
/// interpretation. `raw` keeps the string content of quoted values (and the
/// literal text of numeric ones), so string fields accept any override text
/// while numeric fields use the parsed kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TomlValue<'a> {
    /// String content for quoted values; the literal for everything else.
    pub raw: &'a str,
    /// Typed interpretation of `raw`.
    pub kind: ValueKind,
}

/// The typed shape of a [`TomlValue`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValueKind {
    /// A quoted TOML string (escapes are not part of the parsed subset).
    Str,
    /// An integer literal, decimal or `0x` hex.
    Int(i64),
    /// A decimal float literal.
    Float(f64),
}

impl<'a> TomlValue<'a> {
    /// Interpret an NVS-style raw value: numeric-looking strings become
    /// numbers, everything else is a string. String fields read `raw`
    /// regardless of kind, so an SSID of `12345` still lands as a string.
    fn parse_raw(raw: &'a str) -> Self {
        parse_value(raw).unwrap_or(Self {
            raw,
            kind: ValueKind::Str,
        })
    }
}

/// Flat `section.key → value` table from parsing one TOML text.
///
/// Keys are dotted (`sampling.report_interval_s`), duplicates keep the last
/// occurrence, and entries beyond [`TABLE_MAX`] are dropped — the parser is
/// total, so no config input can fail a boot.
#[derive(Debug)]
pub struct TomlTable<'a> {
    entries: HeapVec<(HeapString<KEY_MAX>, TomlValue<'a>), TABLE_MAX>,
}

impl<'a> TomlTable<'a> {
    /// Value for a dotted key, or `None` when absent.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&TomlValue<'a>> {
        self.entries
            .iter()
            .find(|(k, _)| k.as_str() == key)
            .map(|(_, v)| v)
    }

    /// Every `(key, value)` pair, unknown keys included — callers filter.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &TomlValue<'a>)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    fn insert(&mut self, key: HeapString<KEY_MAX>, value: TomlValue<'a>) {
        for entry in self.entries.iter_mut() {
            if entry.0.as_str() == key.as_str() {
                entry.1 = value; // duplicate key: last one wins
                return;
            }
        }
        // Over TABLE_MAX the entry is dropped; the parser stays total.
        let _ = self.entries.push((key, value));
    }
}

/// Parse the TOML subset the config file uses (see the module docs). Never
/// fails: unparseable lines are skipped.
#[must_use]
pub fn parse_toml(text: &str) -> TomlTable<'_> {
    let mut table = TomlTable {
        entries: HeapVec::new(),
    };
    let mut section = "";
    for line in text.lines() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(inner) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = inner.trim();
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let Some(value) = parse_value(raw_value.trim()) else {
            continue;
        };
        if key.is_empty() {
            continue;
        }
        let sep = usize::from(!section.is_empty());
        if section.len() + sep + key.len() > KEY_MAX {
            continue; // over the documented key cap: skipped
        }
        let mut full = HeapString::new();
        if !section.is_empty() {
            // Length checked above, so these cannot fail.
            full.push_str(section).ok();
            full.push('.').ok();
        }
        full.push_str(key).ok();
        table.insert(full, value);
    }
    table
}

/// Cut a `#` comment, respecting `#` inside quoted strings.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..idx],
            _ => {}
        }
    }
    line
}

/// Parse one value literal: quoted string, decimal/`0x` hex integer, or
/// decimal float. `None` = not in the parsed subset.
fn parse_value(raw: &str) -> Option<TomlValue<'_>> {
    if let Some(inner) = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        return Some(TomlValue {
            raw: inner,
            kind: ValueKind::Str,
        });
    }
    if raw.len() > 2
        && raw[..2].eq_ignore_ascii_case("0x")
        && let Ok(v) = i64::from_str_radix(&raw[2..], 16)
    {
        return Some(TomlValue {
            raw,
            kind: ValueKind::Int(v),
        });
    }
    if let Ok(v) = raw.parse::<i64>() {
        return Some(TomlValue {
            raw,
            kind: ValueKind::Int(v),
        });
    }
    if let Ok(v) = raw.parse::<f64>() {
        return Some(TomlValue {
            raw,
            kind: ValueKind::Float(v),
        });
    }
    None
}

// ---------------------------------------------------------------------------
// Override source — the NVS tier, injectable on the host
// ---------------------------------------------------------------------------

/// Lookup over the device's NVS namespace, abstracted so host tests can
/// inject values and the device personality can plug the real NVS driver
/// without touching this module.
pub trait OverrideSource {
    /// Raw value for a dotted key, or `None`. Implementations return `None`
    /// for keys outside their namespace — including unknown keys, which is
    /// how tolerance works for the override tier.
    fn get(&self, key: &str) -> Option<&str>;
}

/// The empty override source: every lookup misses. What a fresh,
/// unprovisioned flash boots with (file tier over built-ins), and what host
/// tests use to isolate the file tier.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOverrides;

impl OverrideSource for NoOverrides {
    fn get(&self, _key: &str) -> Option<&str> {
        None
    }
}

/// A fixed key/value table as an [`OverrideSource`] — the host-test
/// stand-in for NVS, and usable by the provisioning flow to replay a batch
/// of writes.
#[derive(Debug, Clone, Copy)]
pub struct StaticOverrides<'a> {
    entries: &'a [(&'a str, &'a str)],
}

impl<'a> StaticOverrides<'a> {
    /// Wrap a table of dotted keys and raw values.
    #[must_use]
    pub const fn new(entries: &'a [(&'a str, &'a str)]) -> Self {
        Self { entries }
    }
}

impl OverrideSource for StaticOverrides<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
    }
}

// ---------------------------------------------------------------------------
// Resolved configuration
// ---------------------------------------------------------------------------

/// Which compiled transport sends the events. A config edit can only select
/// among transports compiled in — enabling a missing one is the cargo-flag
/// half's job (`features::resolve_transport`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTransport {
    /// HTTP JSON ingest (the today's-server transport).
    Http,
    /// MQTT v2 protobuf contract.
    Mqtt,
}

impl ActiveTransport {
    /// The config-file spelling of this transport.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Mqtt => "mqtt",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "http" => Some(Self::Http),
            "mqtt" => Some(Self::Mqtt),
            _ => None,
        }
    }
}

/// `[sampling]` — measurement and reporting cadence.
#[derive(Debug, Clone, PartialEq)]
pub struct Sampling {
    /// Length of one decibel measurement window in ms; the 125 ms Fast
    /// detector runs inside it.
    pub decibel_window_ms: u32,
    /// Per-type reporting cadence in seconds.
    pub report_interval_s: u64,
    /// Measurements held while disconnected.
    pub offline_buffer: u32,
}

impl Sampling {
    /// The reporting cadence in ms — bridges to [`REPORT_INTERVAL_MS`].
    #[must_use]
    pub const fn report_interval_ms(&self) -> u64 {
        self.report_interval_s * 1000
    }
}

/// `[sensors.*]` — per-sensor hardware attachment points.
#[derive(Debug, Clone, PartialEq)]
pub struct Sensors {
    /// `[sensors.bme280]` — I2C bus address of the temperature/humidity/
    /// pressure sensor.
    pub bme280: Bme280,
    /// `[sensors.pir]` — GPIO pin of the occupancy front-end.
    pub pir: Pir,
}

/// `[sensors.bme280]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Bme280 {
    /// I2C bus address.
    pub i2c_address: u8,
}

/// `[sensors.pir]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pir {
    /// GPIO pin.
    pub gpio_pin: u8,
}

/// `[transport]` — where events go and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Transport {
    /// Which compiled transport is active.
    pub active: ActiveTransport,
    /// HTTP JSON ingest endpoint.
    pub http_endpoint: HeapString<128>,
    /// `[transport.mqtt]`.
    pub mqtt: Mqtt,
}

/// `[transport.mqtt]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Mqtt {
    /// Broker host.
    pub broker: HeapString<128>,
    /// Broker port — 8883 for TLS, plain 1883 only without mTLS.
    pub port: u16,
}

/// `[decibel]` — sound measurement tuning.
#[derive(Debug, Clone, PartialEq)]
pub struct Decibel {
    /// Top of the mic's measurement range; readings past it flag overload.
    pub overload_threshold_db: f32,
    /// Per-unit calibration from NVS at runtime.
    pub calibration_offset_db: f32,
}

/// The fully resolved, secret-free runtime configuration: built-ins merged
/// with `config/default.toml` and NVS overrides. Serializing or `Debug`-
/// printing a `Config` is safe — secrets live in [`Secrets`], never here.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// `[sensor] id` — device identity UUID.
    pub sensor_id: HeapString<36>,
    /// `[sampling]`.
    pub sampling: Sampling,
    /// `[sensors.*]`.
    pub sensors: Sensors,
    /// `[transport]`.
    pub transport: Transport,
    /// `[decibel]`.
    pub decibel: Decibel,
}

impl Config {
    /// Merge all three tiers: built-in constants, then the TOML file, then
    /// NVS overrides. Tolerates everything — unknown keys, unparseable or
    /// wrong-typed values, oversized strings — by keeping the lower tier's
    /// value, so a bad NVS write cannot fail a boot.
    #[must_use]
    pub fn from_tiers(default_toml: &str, overrides: &dyn OverrideSource) -> Self {
        let mut config = Self::builtin();
        for (key, value) in parse_toml(default_toml).iter() {
            config.set_field(key, value);
        }
        for key in CONFIG_KEYS {
            if let Some(raw) = overrides.get(key) {
                config.set_field(key, &TomlValue::parse_raw(raw));
            }
        }
        config
    }

    /// The bottom tier alone: built-in constants, no file, no NVS.
    #[must_use]
    pub fn builtin() -> Self {
        fn text<const C: usize>(value: &str) -> HeapString<C> {
            let mut slot = HeapString::<C>::new();
            slot.push_str(value)
                .expect("built-in literal fits its capacity");
            slot
        }
        Self {
            sensor_id: text::<36>(SENSOR_ID),
            sampling: Sampling {
                decibel_window_ms: DECIBEL_WINDOW_MS,
                report_interval_s: REPORT_INTERVAL_MS / 1000,
                offline_buffer: OFFLINE_BUFFER,
            },
            sensors: Sensors {
                bme280: Bme280 {
                    i2c_address: BME280_I2C_ADDRESS,
                },
                pir: Pir {
                    gpio_pin: PIR_GPIO_PIN,
                },
            },
            transport: Transport {
                active: ActiveTransport::parse(ACTIVE_TRANSPORT)
                    .expect("built-in transport name is valid"),
                http_endpoint: text::<128>(HTTP_ENDPOINT),
                mqtt: Mqtt {
                    broker: text::<128>(MQTT_BROKER),
                    port: MQTT_PORT,
                },
            },
            decibel: Decibel {
                overload_threshold_db: OVERLOAD_THRESHOLD_DB,
                calibration_offset_db: CALIBRATION_OFFSET_DB,
            },
        }
    }

    /// Apply one key/value pair. Unknown keys fall through the catch-all:
    /// the file tier iterates every parsed pair, the override tier only the
    /// known keys — either way, nothing outside this match can enter a
    /// `Config`, which is the structural half of the secret exclusion.
    fn set_field(&mut self, key: &str, value: &TomlValue<'_>) {
        match key {
            "sensor.id" => put_str(&mut self.sensor_id, value.raw),
            "sampling.decibel_window_ms" => {
                put_u32(&mut self.sampling.decibel_window_ms, value.kind);
            }
            "sampling.report_interval_s" => {
                put_u64(&mut self.sampling.report_interval_s, value.kind);
            }
            "sampling.offline_buffer" => {
                put_u32(&mut self.sampling.offline_buffer, value.kind);
            }
            "sensors.bme280.i2c_address" => {
                put_u8(&mut self.sensors.bme280.i2c_address, value.kind);
            }
            "sensors.pir.gpio_pin" => {
                put_u8(&mut self.sensors.pir.gpio_pin, value.kind);
            }
            "transport.active" => {
                if let Some(active) = ActiveTransport::parse(value.raw) {
                    self.transport.active = active;
                }
            }
            "transport.http_endpoint" => {
                put_str(&mut self.transport.http_endpoint, value.raw);
            }
            "transport.mqtt.broker" => {
                put_str(&mut self.transport.mqtt.broker, value.raw);
            }
            "transport.mqtt.port" => {
                put_u16(&mut self.transport.mqtt.port, value.kind);
            }
            "decibel.overload_threshold_db" => {
                put_f32(&mut self.decibel.overload_threshold_db, value.kind);
            }
            "decibel.calibration_offset_db" => {
                put_f32(&mut self.decibel.calibration_offset_db, value.kind);
            }
            _ => {} // unknown key: tolerated by design (host-tested)
        }
    }

    /// Serialize as TOML — same structure and key names as
    /// `config/default.toml`. Secret keys are structurally absent, so the
    /// output is safe for logs and diagnostics.
    #[must_use]
    pub fn to_toml(&self) -> HeapString<CONFIG_TOML_MAX> {
        let mut out = HeapString::new();
        // CONFIG_TOML_MAX is compile-time and the round-trip test pins
        // adequacy; a write that would overflow is dropped.
        write!(
            out,
            "[sensor]\nid = \"{}\"\n\n[sampling]\ndecibel_window_ms = {}\n\
             report_interval_s = {}\noffline_buffer = {}\n\n[sensors.bme280]\n\
             i2c_address = {}\n\n[sensors.pir]\ngpio_pin = {}\n\n[transport]\n\
             active = \"{}\"\nhttp_endpoint = \"{}\"\n\n[transport.mqtt]\n\
             broker = \"{}\"\nport = {}\n\n[decibel]\n\
             overload_threshold_db = ",
            self.sensor_id.as_str(),
            self.sampling.decibel_window_ms,
            self.sampling.report_interval_s,
            self.sampling.offline_buffer,
            self.sensors.bme280.i2c_address,
            self.sensors.pir.gpio_pin,
            self.transport.active.as_str(),
            self.transport.http_endpoint.as_str(),
            self.transport.mqtt.broker.as_str(),
            self.transport.mqtt.port,
        )
        .ok();
        write_float(&mut out, self.decibel.overload_threshold_db);
        writeln!(out).ok();
        write!(out, "calibration_offset_db = ").ok();
        write_float(&mut out, self.decibel.calibration_offset_db);
        writeln!(out).ok();
        out
    }
}

/// Write a float as a TOML float literal: whole numbers get a `.0` suffix so
/// the field type survives a parse; non-finite values print as-is (`inf`,
/// `NaN` carry letters and so escape the suffix).
fn write_float(out: &mut HeapString<CONFIG_TOML_MAX>, value: f32) {
    let mut text = HeapString::<32>::new();
    write!(text, "{value}").ok();
    if text.chars().all(|c| c.is_ascii_digit() || c == '-') {
        text.push_str(".0").ok();
    }
    out.push_str(text.as_str()).ok();
}

/// Set a string field from a parsed value's text. An oversized value is
/// ignored — the lower tier stands — rather than silently truncated.
fn put_str<const C: usize>(slot: &mut HeapString<C>, raw: &str) {
    if raw.len() <= C {
        slot.clear();
        // Capacity checked above, so this cannot fail.
        slot.push_str(raw).ok();
    }
}

fn put_u8(slot: &mut u8, kind: ValueKind) {
    if let ValueKind::Int(v) = kind
        && (0..=i64::from(u8::MAX)).contains(&v)
    {
        *slot = v as u8;
    }
}

fn put_u16(slot: &mut u16, kind: ValueKind) {
    if let ValueKind::Int(v) = kind
        && (0..=i64::from(u16::MAX)).contains(&v)
    {
        *slot = v as u16;
    }
}

fn put_u32(slot: &mut u32, kind: ValueKind) {
    if let ValueKind::Int(v) = kind
        && (0..=i64::from(u32::MAX)).contains(&v)
    {
        *slot = v as u32;
    }
}

fn put_u64(slot: &mut u64, kind: ValueKind) {
    if let ValueKind::Int(v) = kind
        && v >= 0
    {
        *slot = v as u64;
    }
}

/// Float fields accept both float and integer literals (`120` and `120.0`
/// both mean 120.0).
fn put_f32(slot: &mut f32, kind: ValueKind) {
    match kind {
        ValueKind::Float(v) => *slot = v as f32,
        ValueKind::Int(v) => *slot = v as f32,
        ValueKind::Str => {}
    }
}

// ---------------------------------------------------------------------------
// Secrets — NVS-only, structurally excluded from Config
// ---------------------------------------------------------------------------

/// The device's secret material, read from NVS and nowhere else.
///
/// Not a field of [`Config`], so no config serialization or `Debug` can
/// carry it. This type's own `Debug` is redacted to `set`/`empty` per field —
/// a `println!("{secrets:?}")` in the device personality cannot leak values.
#[derive(Clone)]
pub struct Secrets {
    /// Wi-Fi network name (802.11: at most 32 bytes).
    pub wifi_ssid: HeapString<32>,
    /// Wi-Fi password (WPA2: at most 63 characters).
    pub wifi_password: HeapString<64>,
    /// Ingest API key (`apikey` field of the HTTP contract).
    pub api_key: HeapString<128>,
    /// Client certificate for mTLS (PEM). mTLS streaming of longer chains is
    /// a transport-task concern; this holds the initial material.
    pub client_cert: HeapString<512>,
    /// Client private key for mTLS (PEM).
    pub client_key: HeapString<512>,
}

impl Secrets {
    /// A fresh, empty secret set — what an unprovisioned device holds.
    pub const EMPTY: Self = Self {
        wifi_ssid: HeapString::new(),
        wifi_password: HeapString::new(),
        api_key: HeapString::new(),
        client_cert: HeapString::new(),
        client_key: HeapString::new(),
    };

    /// Pull every secret key from the override source (NVS on device).
    /// Absent or oversized values leave the field empty; a secret key in a
    /// TOML file cannot reach here because only this constructor reads
    /// [`SECRET_KEYS`] and it only takes an [`OverrideSource`].
    #[must_use]
    pub fn from_overrides(overrides: &dyn OverrideSource) -> Self {
        let mut secrets = Self::EMPTY;
        // Per-field calls, not a slot array: the five fields have different
        // capacities (32/64/128/512), so each gets its own type-checked
        // bound. A secret key in a TOML file cannot reach here because only
        // this constructor reads SECRET_KEYS and it only takes an
        // OverrideSource.
        if let Some(raw) = overrides.get("secret.wifi_ssid") {
            put_str(&mut secrets.wifi_ssid, raw);
        }
        if let Some(raw) = overrides.get("secret.wifi_password") {
            put_str(&mut secrets.wifi_password, raw);
        }
        if let Some(raw) = overrides.get("secret.api_key") {
            put_str(&mut secrets.api_key, raw);
        }
        if let Some(raw) = overrides.get("secret.client_cert") {
            put_str(&mut secrets.client_cert, raw);
        }
        if let Some(raw) = overrides.get("secret.client_key") {
            put_str(&mut secrets.client_key, raw);
        }
        secrets
    }
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secrets { ")?;
        for (idx, (name, value)) in [
            ("wifi_ssid", self.wifi_ssid.as_str()),
            ("wifi_password", self.wifi_password.as_str()),
            ("api_key", self.api_key.as_str()),
            ("client_cert", self.client_cert.as_str()),
            ("client_key", self.client_key.as_str()),
        ]
        .into_iter()
        .enumerate()
        {
            if idx > 0 {
                f.write_str(", ")?;
            }
            write!(
                f,
                "{name}: {}",
                if value.is_empty() { "empty" } else { "set" }
            )?;
        }
        f.write_str(" }")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_depths_are_bounded_and_sane() {
        assert!((1..=1024).contains(&MEASUREMENT_QUEUE_DEPTH));
        assert!((1..=1024).contains(&STATUS_QUEUE_DEPTH));
    }

    #[test]
    fn builtins_mirror_config_default_toml() {
        // Keep in sync with config/default.toml — its comments say so too.
        assert_eq!(REPORT_INTERVAL_MS, 60 * 1000);
        assert_eq!(DECIBEL_WINDOW_MS, 1000);
        assert!(matches!(ACTIVE_TRANSPORT, "http" | "mqtt"));
    }

    #[test]
    fn parser_reads_the_committed_file() {
        let table = parse_toml(DEFAULT_TOML);
        assert_eq!(table.iter().count(), CONFIG_KEYS.len());
        assert_eq!(table.get("sensor.id").map(|v| v.raw), Some(SENSOR_ID));
        assert_eq!(
            table.get("sampling.report_interval_s").map(|v| v.kind),
            Some(ValueKind::Int(60))
        );
        assert_eq!(
            table.get("sensors.bme280.i2c_address").map(|v| v.kind),
            Some(ValueKind::Int(0x76))
        );
        assert_eq!(
            table.get("decibel.overload_threshold_db").map(|v| v.kind),
            Some(ValueKind::Float(120.0))
        );
        assert_eq!(
            table.get("transport.active").map(|v| v.kind),
            Some(ValueKind::Str)
        );
        // A key that is not in the file must not be invented either.
        assert!(table.get("sampling.nonsense").is_none());
    }

    #[test]
    fn parser_keeps_hash_inside_strings_and_drops_duplicates() {
        let text = "k = \"x#y\" # trailing comment\n\
                    [dup]\nreport_interval_s = 1\n\
                    report_interval_s = 9\n";
        let table = parse_toml(text);
        assert_eq!(table.get("k").map(|v| v.raw), Some("x#y"));
        // Duplicate key: last occurrence wins.
        assert_eq!(
            table.get("dup.report_interval_s").map(|v| v.kind),
            Some(ValueKind::Int(9))
        );
    }
}
