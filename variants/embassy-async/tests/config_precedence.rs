//! Host tests for the three-tier configuration rule (spec §Configuration):
//! an NVS value beats the `config/default.toml` value, which beats the
//! built-in constant. Also here: unknown-key tolerance, secret exclusion
//! from serialized config, and a serialization round-trip.
//!
//! The override tier is injected (`StaticOverrides`), so none of this needs
//! hardware — the same reason the device personality can resolve config
//! without an NVS driver in the loop of every test.

use clubins_firmware_v3::config::{
    ActiveTransport, CONFIG_KEYS, Config, DEFAULT_TOML, NoOverrides, SECRET_KEYS,
    Secrets, StaticOverrides, parse_toml,
};

/// A file tier that differs from the built-ins on every section, so any
/// tier-ordering mistake shows up as a wrong value.
const FILE: &str = "[sensor]\n\
                    id = \"11111111-2222-3333-4444-555555555555\"\n\
                    \n\
                    [sampling]\n\
                    decibel_window_ms = 2000\n\
                    report_interval_s = 15\n\
                    offline_buffer = 8\n\
                    \n\
                    [sensors.bme280]\n\
                    i2c_address = 0x77\n\
                    \n\
                    [sensors.pir]\n\
                    gpio_pin = 5\n\
                    \n\
                    [transport]\n\
                    active = \"mqtt\"\n\
                    http_endpoint = \"https://example.test/ingest\"\n\
                    \n\
                    [transport.mqtt]\n\
                    broker = \"mqtt2.example.test\"\n\
                    port = 8884\n\
                    \n\
                    [decibel]\n\
                    overload_threshold_db = 115.5\n\
                    calibration_offset_db = -1.5\n";

fn file_only() -> Config {
    Config::from_tiers(FILE, &NoOverrides)
}

#[test]
fn builtin_is_the_bottom_tier() {
    // The committed file mirrors the built-ins, so no-override resolution
    // must reproduce the built-in tier exactly.
    assert_eq!(
        Config::from_tiers(DEFAULT_TOML, &NoOverrides),
        Config::builtin()
    );
    let builtin = Config::builtin();
    assert_eq!(builtin.sampling.report_interval_ms(), 60_000);
    assert_eq!(builtin.transport.active, ActiveTransport::Http);
}

#[test]
fn file_overrides_builtins() {
    let cfg = file_only();
    assert_eq!(
        cfg.sensor_id.as_str(),
        "11111111-2222-3333-4444-555555555555"
    );
    assert_eq!(cfg.sampling.decibel_window_ms, 2000);
    assert_eq!(cfg.sampling.report_interval_s, 15);
    assert_eq!(cfg.sampling.offline_buffer, 8);
    assert_eq!(cfg.sensors.bme280.i2c_address, 0x77);
    assert_eq!(cfg.sensors.pir.gpio_pin, 5);
    assert_eq!(cfg.transport.active, ActiveTransport::Mqtt);
    assert_eq!(
        cfg.transport.http_endpoint.as_str(),
        "https://example.test/ingest"
    );
    assert_eq!(cfg.transport.mqtt.broker.as_str(), "mqtt2.example.test");
    assert_eq!(cfg.transport.mqtt.port, 8884);
    assert!((cfg.decibel.overload_threshold_db - 115.5).abs() < f32::EPSILON);
    assert!((cfg.decibel.calibration_offset_db - (-1.5)).abs() < f32::EPSILON);
}

#[test]
fn nvs_overrides_file_and_builtins() {
    let overrides = StaticOverrides::new(&[
        ("sensor.id", "99999999-8888-7777-6666-555555555555"),
        ("sampling.report_interval_s", "5"),
        ("sensors.pir.gpio_pin", "6"),
        ("sensors.bme280.i2c_address", "0x78"),
        ("decibel.calibration_offset_db", "-2.25"),
        ("transport.mqtt.port", "8885"),
    ]);
    let cfg = Config::from_tiers(FILE, &overrides);
    // NVS wins over the file tier.
    assert_eq!(
        cfg.sensor_id.as_str(),
        "99999999-8888-7777-6666-555555555555"
    );
    assert_eq!(cfg.sampling.report_interval_s, 5);
    assert_eq!(cfg.sensors.pir.gpio_pin, 6);
    assert_eq!(cfg.sensors.bme280.i2c_address, 0x78);
    assert!((cfg.decibel.calibration_offset_db - (-2.25)).abs() < f32::EPSILON);
    assert_eq!(cfg.transport.mqtt.port, 8885);
    // Fields NVS is silent on keep their file-tier values.
    assert_eq!(cfg.sampling.decibel_window_ms, 2000);
    assert_eq!(cfg.sampling.offline_buffer, 8);
    assert_eq!(cfg.transport.active, ActiveTransport::Mqtt);
}

#[test]
fn nvs_overrides_beat_the_embedded_file_on_the_same_key() {
    // One key, three tiers, each different: the NVS value must be visible.
    let overrides = StaticOverrides::new(&[("sampling.report_interval_s", "5")]);
    assert_eq!(Config::builtin().sampling.report_interval_s, 60);
    assert_eq!(
        Config::from_tiers(DEFAULT_TOML, &NoOverrides)
            .sampling
            .report_interval_s,
        60
    );
    assert_eq!(
        Config::from_tiers(DEFAULT_TOML, &overrides)
            .sampling
            .report_interval_s,
        5
    );
}

#[test]
fn unknown_keys_are_tolerated() {
    let text = "[section.from.the.future]\n\
                mystery = \"value\"\n\
                \n\
                [sampling]\n\
                report_interval_s = 7\n\
                nonsense_key = true_not_parsed_either\n\
                \n\
                [transport]\n\
                active = \"http\"\n";
    let overrides = StaticOverrides::new(&[
        ("sampling.nonsense", "7"),
        ("brand.new.section.key", "x"),
        ("secret.nonsense", "y"),
    ]);
    // Unknown keys in either tier must neither fail the load nor leak into
    // known fields.
    let cfg = Config::from_tiers(text, &overrides);
    assert_eq!(cfg.sampling.report_interval_s, 7);
    assert_eq!(cfg.transport.active, ActiveTransport::Http);
    // Untouched fields keep their built-in values.
    assert_eq!(cfg.sampling.offline_buffer, 64);
}

#[test]
fn malformed_and_oversized_values_fall_back() {
    let long_endpoint = format!("https://{}", "x".repeat(200));
    let text = "[sampling]\n\
                report_interval_s = \"soon\"\n\
                decibel_window_ms = [1, 2]\n\
                \n\
                [sensors.pir]\n\
                gpio_pin = 99999\n\
                \n\
                [decibel]\n\
                overload_threshold_db = \"loud\"\n";
    // Bound to a local: one entry borrows `long_endpoint`, so the array
    // cannot be const-promoted and must outlive `overrides`.
    let entries = [
        ("transport.active", "carrier-pigeon"),
        ("transport.http_endpoint", long_endpoint.as_str()),
        ("sensors.bme280.i2c_address", "-1"),
    ];
    let overrides = StaticOverrides::new(&entries);
    let cfg = Config::from_tiers(text, &overrides);
    // Wrong-typed, out-of-range, and oversized values: the lower tier stands.
    assert_eq!(cfg.sampling.report_interval_s, 60);
    assert_eq!(cfg.sampling.decibel_window_ms, 1000);
    assert_eq!(cfg.sensors.pir.gpio_pin, 4);
    assert_eq!(cfg.sensors.bme280.i2c_address, 0x76);
    assert!((cfg.decibel.overload_threshold_db - 120.0).abs() < f32::EPSILON);
    assert_eq!(cfg.transport.active, ActiveTransport::Http);
    assert_eq!(
        cfg.transport.http_endpoint.as_str(),
        "https://ingest.clubins.de/v1/sensors"
    );
}

/// Canary marker used to prove a value never reaches serialized output.
const CANARIES: [(&str, &str); 5] = [
    ("secret.wifi_ssid", "CANARY-wifi-ssid"),
    ("secret.wifi_password", "CANARY-wifi-password"),
    ("secret.api_key", "CANARY-api-key"),
    ("secret.client_cert", "CANARY-client-cert"),
    ("secret.client_key", "CANARY-client-key"),
];

#[test]
fn secrets_come_only_from_overrides() {
    let sneaky = "[secret]\n\
                  api_key = \"CANARY-in-file\"\n\
                  wifi_password = \"CANARY-in-file\"\n";
    let cfg = Config::from_tiers(sneaky, &NoOverrides);
    // Secret keys in the file tier are ignored outright…
    assert!(!cfg.to_toml().contains("CANARY-in-file"));
    // …and with no override source, every secret field stays empty.
    let secrets = Secrets::from_overrides(&NoOverrides);
    assert_eq!(secrets.wifi_ssid.as_str(), "");
    assert_eq!(secrets.wifi_password.as_str(), "");
    assert_eq!(secrets.api_key.as_str(), "");
    assert_eq!(secrets.client_cert.as_str(), "");
    assert_eq!(secrets.client_key.as_str(), "");
}

#[test]
fn secrets_read_from_overrides_are_kept_out_of_serialized_output() {
    let mut entries: Vec<(&str, &str)> = CANARIES.to_vec();
    entries.push(("sampling.report_interval_s", "5"));
    let overrides = StaticOverrides::new(&entries);
    let cfg = Config::from_tiers(DEFAULT_TOML, &overrides);
    let secrets = Secrets::from_overrides(&overrides);
    // The secrets ARE read from the override tier…
    assert_eq!(secrets.wifi_ssid.as_str(), "CANARY-wifi-ssid");
    assert_eq!(secrets.api_key.as_str(), "CANARY-api-key");
    // …but no serialized surface carries any of them: not the TOML dump,
    // not the config's Debug, not the redacted Secrets Debug.
    let dump = cfg.to_toml();
    for (_, canary) in CANARIES {
        assert!(!dump.contains(canary), "leaked into to_toml: {canary}");
        assert!(
            !format!("{cfg:?}").contains(canary),
            "leaked into Config Debug: {canary}"
        );
        assert!(
            !format!("{secrets:?}").contains(canary),
            "leaked into Secrets Debug: {canary}"
        );
    }
}

#[test]
fn oversized_secret_is_ignored_not_truncated() {
    let long_ssid = "y".repeat(40); // 802.11 caps SSIDs at 32 bytes
    let entries = [("secret.wifi_ssid", long_ssid.as_str())];
    let overrides = StaticOverrides::new(&entries);
    let secrets = Secrets::from_overrides(&overrides);
    assert_eq!(secrets.wifi_ssid.as_str(), "");
}

#[test]
fn serialized_config_round_trips() {
    let overrides = StaticOverrides::new(&[
        ("sensor.id", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
        ("sampling.report_interval_s", "15"),
        ("sampling.offline_buffer", "32"),
        ("sensors.bme280.i2c_address", "0x77"),
        ("transport.active", "mqtt"),
        ("decibel.overload_threshold_db", "115.5"),
        ("decibel.calibration_offset_db", "-2.25"),
    ]);
    let cfg = Config::from_tiers(DEFAULT_TOML, &overrides);
    let text = cfg.to_toml();
    // Parsing the serialization back (no overrides) must reproduce the
    // config exactly — every field survives the writer.
    assert_eq!(Config::from_tiers(text.as_str(), &NoOverrides), cfg);
    // And the serialization must still parse under the same subset rules.
    assert_eq!(parse_toml(text.as_str()).iter().count(), CONFIG_KEYS.len());
}

#[test]
fn committed_file_defines_every_key_and_no_secrets() {
    let table = parse_toml(DEFAULT_TOML);
    for (key, _) in table.iter() {
        assert!(
            !key.starts_with("secret"),
            "secret key in default.toml: {key}"
        );
    }
    for key in CONFIG_KEYS {
        assert!(table.get(key).is_some(), "{key} missing from default.toml");
    }
    // The override source must not be consulted for anything outside the
    // two key lists.
    for key in SECRET_KEYS {
        assert!(key.starts_with("secret."));
    }
}

#[test]
fn secret_keys_live_outside_config_keys() {
    for secret in SECRET_KEYS {
        assert!(!CONFIG_KEYS.contains(&secret));
    }
}
