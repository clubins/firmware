//! Config resolution end-to-end: the precedence ladder the boot walks.
//!
//! These tests exercise the public surface the way the device will: parse
//! the committed `config/default.toml` (the real file in the crate, not a
//! copy), apply NVS overrides through an injectable source, and check the
//! four behaviors the spec pins (§Configuration):
//!
//! - precedence: NVS value → default.toml value → built-in constant;
//! - unknown-key tolerance: tolerated with a visible warn list;
//! - secret exclusion: secrets never serialize, from any rung;
//! - round-trip parse: resolve → serialize → parse → the same config.
//!
//! No feature gate: the config and nvs modules are unconditional pure
//! logic, so this suite runs in every flag world CI builds.

use firmware_v8_declarative::config::{
    self, Config, DecibelConfig, Parsed, SamplingConfig, SensorConfig, SensorsConfig,
    TransportConfig, Warnings,
};
use firmware_v8_declarative::nvs::{
    self, Namespace, NoOverrides, OverrideSource, VecOverrides,
};

/// The committed document exactly as the device embeds it.
const COMMITTED: &str = include_str!("../config/default.toml");

/// The boot's resolution order, as one function: the committed document
/// over the built-ins, then NVS overrides on top, with one merged warn
/// list from both rungs.
fn resolve(document: &str, overrides: &dyn OverrideSource) -> (Config, Warnings) {
    let Parsed {
        config: mut resolved,
        warnings: mut merged,
    } = Config::parse(document).expect("document parses");
    nvs::apply(&mut resolved, overrides, &mut merged);
    (resolved, merged)
}

fn overrides(entries: &[(Namespace, &str, &str)]) -> VecOverrides {
    VecOverrides(
        entries
            .iter()
            .map(|(ns, k, v)| (*ns, (*k).into(), (*v).into()))
            .collect(),
    )
}

/// The spec §Configuration example, verbatim: the values a fresh flash
/// boots with.
fn spec_defaults() -> Config {
    Config {
        sensor: SensorConfig {
            id: "00000000-0000-0000-0000-000000000000".into(),
        },
        sampling: SamplingConfig {
            decibel_window_ms: 1000,
            report_interval_s: 60,
            offline_buffer: 64,
        },
        sensors: SensorsConfig {
            bme280: config::Bme280Config { i2c_address: 0x76 },
            pir: config::PirConfig { gpio_pin: 4 },
        },
        transport: TransportConfig {
            active: config::TransportKind::Http,
            http_endpoint: "https://ingest.clubins.de/v1/sensors".into(),
            mqtt: config::MqttConfig {
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

#[test]
fn committed_document_resolves_to_the_spec_defaults() {
    let (config, warnings) = resolve(COMMITTED, &NoOverrides);
    assert_eq!(config, spec_defaults());
    assert!(warnings.is_empty(), "committed file parses clean");
}

#[test]
fn nvs_beats_document_beats_builtin() {
    // The built-in rung stands alone.
    assert_eq!(Config::builtin().sampling.report_interval_s, 60);

    // The document rung beats it.
    let (config, _) = resolve(
        r#"
        [sampling]
        report_interval_s = 90
        "#,
        &NoOverrides,
    );
    assert_eq!(config.sampling.report_interval_s, 90);

    // The NVS rung beats the document — and a field the overrides do not
    // mention keeps its document value.
    let (config, warnings) = resolve(
        r#"
        [sampling]
        report_interval_s = 90
        offline_buffer = 128
        "#,
        &overrides(&[(Namespace::Schema, "sampling.report_interval_s", "30")]),
    );
    assert!(warnings.is_empty());
    assert_eq!(config.sampling.report_interval_s, 30);
    assert_eq!(config.sampling.offline_buffer, 128);
}

#[test]
fn overrides_apply_to_the_committed_document() {
    let (config, warnings) = resolve(
        COMMITTED,
        &overrides(&[
            (
                Namespace::Identity,
                "sensor_id",
                "0b1ac1e7-38f8-4f3e-9d0a-8b6a2c4e5f70",
            ),
            (Namespace::Cal, "decibel_calibration_offset_db", "-2.5"),
            (Namespace::Schema, "sensors.pir.gpio_pin", "17"),
        ]),
    );
    assert!(warnings.is_empty());
    assert_eq!(config.sensor.id, "0b1ac1e7-38f8-4f3e-9d0a-8b6a2c4e5f70");
    assert_eq!(config.decibel.calibration_offset_db, -2.5);
    assert_eq!(config.sensors.pir.gpio_pin, 17);
    // Untouched fields keep their committed values.
    assert_eq!(config.sampling.report_interval_s, 60);
    assert_eq!(config.transport.mqtt.port, 8883);
}

#[test]
fn unknown_keys_from_both_rungs_land_in_one_warn_list() {
    let (config, warnings) = resolve(
        r#"
        [samplng]
        report_interval_s = 30

        [sampling]
        report_intervall = 30
        "#,
        &overrides(&[
            (Namespace::Schema, "sampling.offline_bufferz", "8"),
            (Namespace::Identity, "serial", "x"),
        ]),
    );
    // The known rungs stand; every typo is named.
    assert_eq!(config.sampling.report_interval_s, 60);
    assert_eq!(config.sampling.offline_buffer, 64);
    assert_eq!(warnings.len(), 4, "one warning per unknown key");
    let joined = warnings.iter().collect::<Vec<_>>().join("\n");
    assert!(joined.contains("[samplng]"), "unknown section is named");
    assert!(
        joined.contains("report_intervall"),
        "unknown doc key is named"
    );
    assert!(
        joined.contains("offline_bufferz"),
        "unknown nvs key is named"
    );
    assert!(joined.contains("serial"), "unknown identity key is named");
}

#[test]
fn bad_nvs_values_never_abandon_the_tested_config() {
    let (config, warnings) = resolve(
        COMMITTED,
        &overrides(&[
            (Namespace::Identity, "sensor_id", "not-a-uuid"),
            (Namespace::Cal, "decibel_calibration_offset_db", "999"),
            (Namespace::Schema, "sampling.report_interval_s", "abc"),
            (Namespace::Schema, "transport.mqtt.port", "70000"),
        ]),
    );
    // Every skip is explained…
    assert_eq!(warnings.len(), 4);
    // …and the resolved config is exactly the committed one.
    assert_eq!(config, spec_defaults());
}

#[test]
fn wire_secrets_never_reach_the_serializable_config() {
    // A hostile provisioning dump: secrets in the wire namespace, plus
    // secret-named keys smuggled into namespaces the resolver reads.
    let (config, warnings) = resolve(
        COMMITTED,
        &overrides(&[
            (Namespace::Wire, "wifi_ssid", "venue-guest"),
            (Namespace::Wire, "wifi_password", "hunter2"),
            (Namespace::Wire, "api_key", "sk-0123456789abcdef"),
            (
                Namespace::Wire,
                "client_cert",
                "-----BEGIN CERTIFICATE-----",
            ),
            (Namespace::Identity, "password", "hunter2"),
            (Namespace::Schema, "wifi_psk", "hunter2"),
        ]),
    );
    // Every wire entry is acknowledged (read by the transport subsystem,
    // not silently dropped); every smuggled key is reported as unknown.
    assert_eq!(warnings.len(), 6);
    // The config is untouched and its serialization is clean.
    assert_eq!(config, spec_defaults());
    let text = config.to_toml();
    assert!(!config::serialized_output_contains_secret_marker(&text));
}

#[test]
fn the_secret_tripwire_catches_every_marker() {
    // The detector knows each marker on its own — so the debug-assert in
    // `to_toml` cannot be defeated by one marker shape.
    for marker in config::SECRET_MARKERS {
        assert!(
            config::serialized_output_contains_secret_marker(&format!(
                "{marker} = \"x\""
            )),
            "detector misses its own marker '{marker}'"
        );
        assert!(
            config::serialized_output_contains_secret_marker(&format!("A {marker} B")),
            "detector is not case/position-robust for '{marker}'"
        );
    }
    // And it does not fire on innocent config text.
    assert!(!config::serialized_output_contains_secret_marker(
        &spec_defaults().to_toml()
    ));
}

#[test]
fn round_trip_parse_preserves_the_resolved_config() {
    let (config, warnings) = resolve(
        COMMITTED,
        &overrides(&[
            (
                Namespace::Identity,
                "sensor_id",
                "0b1ac1e7-38f8-4f3e-9d0a-8b6a2c4e5f70",
            ),
            (Namespace::Cal, "decibel_calibration_offset_db", "-2.5"),
            (Namespace::Schema, "sampling.report_interval_s", "30"),
        ]),
    );
    assert!(warnings.is_empty());

    let text = config.to_toml();
    let Parsed {
        config: reparsed,
        warnings: reparse_warnings,
    } = Config::parse(&text).expect("serialized config reparses");
    assert!(reparse_warnings.is_empty(), "canonical output parses clean");
    assert_eq!(
        reparsed, config,
        "round-trip preserves every resolved value"
    );
}

#[test]
#[cfg(feature = "transport-http")]
fn the_committed_transport_selection_is_bootable() {
    // The committed document selects http; in worlds where the http
    // transport is compiled in, the boot check passes. (In the all-off
    // anchor this test is compiled out together with the transport.)
    assert!(Config::is_compiled(config::TransportKind::Http));
}
