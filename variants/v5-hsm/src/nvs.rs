//! The NVS override layer — the top of the configuration precedence rule
//! (built-in < `config/default.toml` < NVS), and the secret seam.
//!
//! Two traits keep the two concerns apart:
//!
//! - [`NvsSource`] — per-unit *tuning*. The device implements it over its
//!   nonvolatile store; the host tests fake it in memory. Keys are the
//!   dotted config paths ([`crate::config::TUNABLE_PATHS`]) and each value
//!   is the TOML value text (`"publish.interval_ms"` → `"8000"`,
//!   `"device.sensor_id"` → `"\"6f0a…\""`). Every present key becomes a
//!   one-key layer that runs through the same validation as the file — a
//!   bad NVS value warns and the value beneath stands.
//! - [`SecretStore`] — per-unit *credentials* (Wi-Fi, API key, client
//!   certificates). Deliberately NOT part of the config schema: secrets
//!   are read through this seam at use time and can never enter
//!   `config/default.toml`, a [`Config`], or its serializer.
//!
//! [`load_config`] is the boot seam that chains all three layers and
//! yields the resolved [`Config`] plus the merged warn list.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::config::{Config, ConfigWarning, PartialConfig, TUNABLE_PATHS};

/// Read access to the device's nonvolatile overrides (or a host fake).
///
/// `read` answers one dotted config path: `Ok(None)` = no override for
/// that key; `Ok(Some(text))` = the TOML value text as provisioned; an
/// error means the store itself failed for that key — the layer warns and
/// moves on (tolerance rule).
pub trait NvsSource {
    /// The override text for `key`, if this unit has one.
    fn read(&self, key: &str) -> Result<Option<String>, NvsReadError>;
}

/// Why an NVS read failed. Kept opaque-on-purpose: the boot path only
/// needs to surface it on the warn list, not act on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvsReadError(pub String);

impl core::fmt::Display for NvsReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "NVS read failed: {}", self.0)
    }
}

impl core::error::Error for NvsReadError {}

/// The credential seam: per-unit secrets, read at use time, never part of
/// the config schema. A host fake provisions tests; the device
/// implementation reads NVS (and never serializes into logs).
pub trait SecretStore {
    /// Ingest `apikey` header value (wire contract, HTTP transport).
    fn http_api_key(&self) -> Option<String>;
    /// Wi-Fi credentials for station mode.
    fn wifi_ssid(&self) -> Option<String>;
    fn wifi_password(&self) -> Option<String>;
    /// MQTT broker credentials (v2 transport).
    fn mqtt_username(&self) -> Option<String>;
    fn mqtt_password(&self) -> Option<String>;
    /// Client certificate chain + private key (PEM) for mutual TLS.
    fn client_certificate_pem(&self) -> Option<String>;
    fn client_key_pem(&self) -> Option<String>;
}

/// The NVS layer: one validated one-key layer per present override, in
/// [`TUNABLE_PATHS`] order. Warnings carry the offending key, so the boot
/// log says *which* NVS entry was rejected.
pub fn override_layers<S: NvsSource>(source: &S) -> Vec<PartialConfig> {
    let mut layers = Vec::new();
    for key in TUNABLE_PATHS {
        let text = match source.read(key) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(e) => {
                layers.push(PartialConfig {
                    warnings: vec![ConfigWarning::BadValue {
                        key: String::from(*key),
                        reason: e.to_string(),
                    }],
                    ..PartialConfig::default()
                });
                continue;
            }
        };
        // Parse `key = <text>` with the same subset parser and tolerance
        // the file layer uses — one micro-layer per key. A dotted path
        // must go through a table header: `a.b = v` is a dotted key,
        // which the deliberate subset rejects.
        let src = match key.rsplit_once('.') {
            Some((section, leaf)) => format!("[{section}]\n{leaf} = {text}"),
            None => format!("{key} = {text}"),
        };
        let mut layer = PartialConfig::from_toml(&src);
        layer.warnings = layer
            .warnings
            .into_iter()
            .map(|w| match w {
                ConfigWarning::LayerSyntax(msg) => ConfigWarning::BadValue {
                    key: String::from(*key),
                    reason: format!("NVS value rejected: {msg}"),
                },
                other => other,
            })
            .collect();
        layers.push(layer);
    }
    layers
}

/// Boot seam: chain built-in < `config/default.toml` < NVS overrides and
/// return the resolved configuration with the merged warn list. A dropped
/// or corrupt file layer, a failed NVS read, a malformed override — none
/// of them abort the boot; each degrades to the layer beneath it.
pub fn load_config<S: NvsSource>(source: &S) -> (Config, Vec<ConfigWarning>) {
    let file_layer = PartialConfig::from_toml(crate::config::DEFAULT_TOML);
    let nvs_layers = override_layers(source);
    let mut refs: Vec<&PartialConfig> = vec![&file_layer];
    refs.extend(nvs_layers.iter());
    PartialConfig::from_layers(Config::builtin(), &refs)
}
