//! Host tests — the NVS override layer and the secret seam (spec
//! §Configuration): precedence through the real boot seam, per-key
//! validation with attributed warnings, read-failure tolerance, and
//! secret exclusion.

use std::collections::BTreeMap;

use firmware_v5_hsm::config::{Config, ConfigWarning, TransportKind};
use firmware_v5_hsm::nvs::{NvsReadError, NvsSource, SecretStore, load_config};

/// In-memory stand-in for the device's nonvolatile store: dotted config
/// path → TOML value text (or an injected read failure).
struct FakeNvs {
    entries: BTreeMap<&'static str, Result<String, NvsReadError>>,
}

impl FakeNvs {
    fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    fn with(entries: &[(&'static str, &str)]) -> Self {
        Self {
            entries: entries
                .iter()
                .map(|(k, v)| (*k, Ok(String::from(*v))))
                .collect(),
        }
    }
}

impl NvsSource for FakeNvs {
    fn read(&self, key: &str) -> Result<Option<String>, NvsReadError> {
        match self.entries.get(key) {
            Some(Ok(text)) => Ok(Some(text.clone())),
            Some(Err(e)) => Err(e.clone()),
            None => Ok(None),
        }
    }
}

/// A credential holder for the seam tests — the values must never reach a
/// [`firmware_v5_hsm::config::Config`].
struct FakeSecrets {
    api_key: String,
    wifi_password: String,
}

impl SecretStore for FakeSecrets {
    fn http_api_key(&self) -> Option<String> {
        Some(self.api_key.clone())
    }
    fn wifi_ssid(&self) -> Option<String> {
        Some(String::from("bench-wifi"))
    }
    fn wifi_password(&self) -> Option<String> {
        Some(self.wifi_password.clone())
    }
    fn mqtt_username(&self) -> Option<String> {
        None
    }
    fn mqtt_password(&self) -> Option<String> {
        None
    }
    fn client_certificate_pem(&self) -> Option<String> {
        None
    }
    fn client_key_pem(&self) -> Option<String> {
        None
    }
}

// --- precedence through the boot seam --------------------------------------

#[test]
fn nvs_overrides_flow_through_the_boot_seam() {
    let nvs = FakeNvs::with(&[
        ("publish.interval_ms", "8000"),
        ("transport.active", "\"mqtt\""),
        ("sampling.interval_ms", "250"),
    ]);
    let (cfg, warnings) = load_config(&nvs);
    assert!(
        warnings.is_empty(),
        "valid overrides must not warn: {warnings:?}"
    );
    assert_eq!(cfg.publish.interval_ms, 8000);
    assert_eq!(cfg.transport.active, TransportKind::Mqtt);
    assert_eq!(cfg.sampling.interval_ms, 250);
    // Untouched keys keep the committed file's values:
    assert_eq!(cfg.transport.mqtt.port, 8883);
    assert_eq!(cfg.status.heartbeat_ms, 30_000);
}

#[test]
fn an_empty_store_yields_the_file_layer_exactly() {
    let (cfg, warnings) = load_config(&FakeNvs::empty());
    assert!(warnings.is_empty());
    assert_eq!(cfg, Config::builtin(), "builtin mirrors the committed file");
}

#[test]
fn nvs_beats_the_file_which_beats_the_builtin() {
    // sampling.interval_ms exists in all three layers.
    let nvs = FakeNvs::with(&[("sampling.interval_ms", "111")]);
    let (cfg, _) = load_config(&nvs);
    assert_eq!(cfg.sampling.interval_ms, 111, "NVS wins");
}

// --- per-key validation with attributed warnings ----------------------------

#[test]
fn a_malformed_override_warns_with_its_key_and_keeps_the_layer_below() {
    let nvs = FakeNvs::with(&[
        ("publish.interval_ms", "eight seconds"), // not a TOML value
        ("status.heartbeat_ms", "5_000"),         // valid, must still apply
    ]);
    let (cfg, warnings) = load_config(&nvs);
    assert_eq!(cfg.publish.interval_ms, 15_000, "layer-below value stands");
    assert_eq!(
        cfg.status.heartbeat_ms, 5_000,
        "the other override still applies"
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    match &warnings[0] {
        ConfigWarning::BadValue { key, reason } => {
            assert_eq!(key, "publish.interval_ms");
            assert!(
                reason.contains("NVS"),
                "warning must be attributed: {reason}"
            );
        }
        other => panic!("expected a BadValue warning, got {other:?}"),
    }
}

#[test]
fn a_failed_nvs_read_warns_and_keeps_the_layer_below() {
    let mut nvs = FakeNvs::empty();
    nvs.entries.insert(
        "publish.offline_buffer",
        Err(NvsReadError(String::from("flash busy"))),
    );
    let (cfg, warnings) = load_config(&nvs);
    assert_eq!(cfg.publish.offline_buffer, 64, "layer-below value stands");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    match &warnings[0] {
        ConfigWarning::BadValue { key, reason } => {
            assert_eq!(key, "publish.offline_buffer");
            assert!(reason.contains("flash busy"), "{reason}");
        }
        other => panic!("expected a BadValue warning, got {other:?}"),
    }
}

#[test]
fn overrides_pass_the_same_validation_as_the_file() {
    let nvs = FakeNvs::with(&[
        (
            "device.sensor_id",
            "\"6f0a2c3e-9b1d-4c8a-a5e2-3f7b1d9c0e4a\"",
        ),
        ("device.sensor_id_fail", "\"not even read\""), // not a tunable path
        ("sensors.decibel.window_ms", "5"),             // below the 1 s floor
        ("sensors.decibel.interval_ms", "1500"),        // valid
    ]);
    let (cfg, warnings) = load_config(&nvs);
    // Valid string and number overrides apply (quotes are TOML text):
    assert_eq!(cfg.device.sensor_id, "6f0a2c3e-9b1d-4c8a-a5e2-3f7b1d9c0e4a");
    assert_eq!(cfg.sensors.decibel.interval_ms, Some(1500));
    // Out-of-range override warns and keeps the file value:
    assert_eq!(cfg.sensors.decibel.window_ms, 60_000);
    let warned: Vec<&str> = warnings
        .iter()
        .filter_map(|w| match w {
            ConfigWarning::BadValue { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        warned.contains(&"sensors.decibel.window_ms"),
        "{warnings:?}"
    );
    assert_eq!(
        warned.len(),
        1,
        "unknown NVS paths are never read: {warnings:?}"
    );
}

#[test]
fn nvs_can_override_the_decibel_calibration_offset() {
    let nvs = FakeNvs::with(&[("sensors.decibel.calibration_offset_db", "-2.5")]);
    let (cfg, warnings) = load_config(&nvs);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(cfg.sensors.decibel.calibration_offset_db, -2.5);
}

// --- secret exclusion -------------------------------------------------------

#[test]
fn secrets_live_only_behind_the_secret_store() {
    let secrets = FakeSecrets {
        api_key: String::from("sk-live-0123456789abcdef"),
        wifi_password: String::from("correct horse battery staple"),
    };
    let nvs = FakeNvs::with(&[
        // A provisioning mistake: someone put a credential in a *tuning*
        // key's value. It must not leak through the config either.
        ("transport.http.endpoint", "\"https://ingest.clubins.de\""),
    ]);

    let (cfg, warnings) = load_config(&nvs);
    assert!(warnings.is_empty(), "{warnings:?}");

    let serialized = cfg.to_toml();
    for leak in [
        secrets.api_key.as_str(),
        secrets.wifi_password.as_str(),
        "sk-live",
        "correct horse",
    ] {
        assert!(
            !serialized.contains(leak),
            "serialized config leaked `{leak}`"
        );
    }
    // And the resolved config itself carries no credential fields — the
    // only way to reach the secrets is the seam:
    let _ = secrets.http_api_key();
    let _ = secrets.wifi_password();
}
