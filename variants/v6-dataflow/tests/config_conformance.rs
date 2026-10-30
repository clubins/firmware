//! Config conformance — the spec §Configuration contract, verified on the
//! host: layer precedence (`built-in < file < NVS`), unknown-key tolerance
//! with a warn list, secret exclusion from every serializable form, and
//! parse/render round-tripping of the committed defaults.

use firmware_v6_dataflow::config::{
    Config, ConfigError, DEFAULT_TOML, KNOWN_PATHS, KNOWN_TABLES, MAX_VALUE_LEN,
    RENDER_CAP, Table, WarningKind, Warnings,
};
use firmware_v6_dataflow::nvs::{MemorySource, SECRET_KEYS};

/// A fresh scratch table per call — no test may depend on residue.
fn scratch() -> Table {
    Table::new()
}

fn resolve(
    file: Option<&str>,
    nvs: &MemorySource,
) -> Result<(Config, Warnings), ConfigError> {
    Config::resolve_in(file, nvs, &mut scratch())
}

// ---------------------------------------------------------------------------
// Precedence: built-in < default.toml < NVS
// ---------------------------------------------------------------------------

mod precedence {
    use super::*;

    #[test]
    fn built_in_constants_are_the_floor() {
        let (config, warnings) = resolve(None, &MemorySource::EMPTY).expect("resolves");
        assert_eq!(config, Config::BUILT_IN);
        assert_eq!(warnings.iter().count(), 0);
    }

    #[test]
    fn committed_file_overrides_built_in() {
        // Every value the committed file sets must appear in the resolved
        // config — this is also the drift pin between BUILT_IN and the
        // committed file.
        let (config, warnings) =
            resolve(Some(DEFAULT_TOML), &MemorySource::EMPTY).expect("resolves");
        assert_eq!(warnings.iter().count(), 0, "committed defaults parse clean");
        assert_eq!(
            config.sensor.id.as_str(),
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(config.sampling.decibel_window_ms, 1_000);
        assert_eq!(config.sampling.report_interval_s, 60);
        assert_eq!(config.sampling.offline_buffer, 64);
        assert_eq!(config.sensors.bme280.i2c_address, 0x76);
        assert_eq!(config.sensors.pir.gpio_pin, 4);
        assert_eq!(config.sensors.pms5003.uart_baud, 9_600);
        assert_eq!(config.transport.active.as_str(), "http");
        assert_eq!(
            config.transport.http_endpoint.as_str(),
            "https://ingest.clubins.de/v1/sensors"
        );
        assert_eq!(config.transport.mqtt.broker.as_str(), "mqtt.clubins.de");
        assert_eq!(config.transport.mqtt.port, 8883);
        assert_eq!(config.decibel.overload_threshold_db, 120.0);
        assert_eq!(config.decibel.calibration_offset_db, 0.0);
    }

    #[test]
    fn nvs_overrides_win_over_the_file() {
        let mut nvs = MemorySource::new();
        assert!(nvs.insert("sampling.report_interval_s", "30"));
        assert!(nvs.insert("transport.active", "\"mqtt\""));
        assert!(nvs.insert("decibel.calibration_offset_db", "-1.5"));
        let (config, warnings) = resolve(Some(DEFAULT_TOML), &nvs).expect("resolves");
        assert_eq!(warnings.iter().count(), 0);
        assert_eq!(config.sampling.report_interval_s, 30);
        assert_eq!(config.transport.active.as_str(), "mqtt");
        assert_eq!(config.decibel.calibration_offset_db, -1.5);
        // Values the source does not carry stay on the layer below.
        assert_eq!(config.sampling.decibel_window_ms, 1_000);
    }

    #[test]
    fn nvs_wins_even_without_a_file_layer() {
        let mut nvs = MemorySource::new();
        assert!(nvs.insert("sampling.report_interval_s", "15"));
        let (config, _) = resolve(None, &nvs).expect("resolves");
        assert_eq!(config.sampling.report_interval_s, 15);
        assert_eq!(config.sampling.decibel_window_ms, 1_000); // built-in
    }

    #[test]
    fn every_known_path_is_overridable_via_nvs() {
        // The override loop walks KNOWN_PATHS; a source entry for each must
        // land in the resolved config without a single warning.
        let mut nvs = MemorySource::new();
        for path in KNOWN_PATHS {
            assert!(
                nvs.insert(path, value_for(path)),
                "{path} override must fit"
            );
        }
        let (config, warnings) = resolve(Some(DEFAULT_TOML), &nvs).expect("resolves");
        assert_eq!(warnings.iter().count(), 0, "all overrides are valid");
        assert_eq!(
            config.sensor.id.as_str(),
            "12345678-90ab-4cde-8f01-23456789abcd"
        );
        assert_eq!(config.sampling.decibel_window_ms, 250);
        assert_eq!(config.sampling.report_interval_s, 1);
        assert_eq!(config.sampling.offline_buffer, 8);
        assert_eq!(config.sensors.bme280.i2c_address, 0x77);
        assert_eq!(config.sensors.sht4x.i2c_address, 0x45);
        assert_eq!(config.sensors.scd4x.i2c_address, 0x63);
        assert_eq!(config.sensors.bh1750.i2c_address, 0x5C);
        assert_eq!(config.sensors.pir.gpio_pin, 5);
        assert_eq!(config.sensors.pms5003.uart_baud, 115_200);
        assert_eq!(config.transport.active.as_str(), "mqtt");
        assert_eq!(
            config.transport.http_endpoint.as_str(),
            "https://ingest.example.com/hook"
        );
        assert_eq!(config.transport.mqtt.broker.as_str(), "mqtt.example.com");
        assert_eq!(config.transport.mqtt.port, 1883);
        assert_eq!(config.decibel.overload_threshold_db, 115.0);
        assert_eq!(config.decibel.calibration_offset_db, 0.5);
    }

    /// A valid value per known path (the test's table must stay in step
    /// with the schema — the `unreachable` fires if it drifts). String
    /// values carry TOML quotes; numbers, hex, and floats do not.
    fn value_for(path: &str) -> &'static str {
        match path {
            "sensor.id" => "\"12345678-90ab-4cde-8f01-23456789abcd\"",
            "sampling.decibel_window_ms" => "250",
            "sampling.report_interval_s" => "1",
            "sampling.offline_buffer" => "8",
            "sensors.bme280.i2c_address" => "0x77",
            "sensors.sht4x.i2c_address" => "0x45",
            "sensors.scd4x.i2c_address" => "0x63",
            "sensors.bh1750.i2c_address" => "0x5C",
            "sensors.pir.gpio_pin" => "5",
            "sensors.pms5003.uart_baud" => "115200",
            "transport.active" => "\"mqtt\"",
            "transport.http_endpoint" => "\"https://ingest.example.com/hook\"",
            "transport.mqtt.broker" => "\"mqtt.example.com\"",
            "transport.mqtt.port" => "1883",
            "decibel.overload_threshold_db" => "115.0",
            "decibel.calibration_offset_db" => "0.5",
            _ => unreachable!("KNOWN_PATHS drifted: {path}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Unknown-key tolerance — warned, never fatal, never applied
// ---------------------------------------------------------------------------

mod unknown_keys {
    use super::*;

    #[test]
    fn unknown_keys_and_tables_warn_deterministically() {
        let text = "[sampling]\n\
                    report_interval_s = 45\n\
                    decibel_window_s = 2\n\
                    [no.such.table]\n\
                    key = 1\n";
        let nvs = MemorySource::new();
        let mut t = scratch();
        let (config, warnings) =
            Config::resolve_in(Some(text), &nvs, &mut t).expect("unknowns do not abort");
        assert_eq!(config.sampling.report_interval_s, 45);
        assert_eq!(config.sampling.decibel_window_ms, 1_000); // unknown never applied
        let kinds: Vec<_> = warnings.iter().map(|w| w.kind).collect();
        // Parse-level warnings (unknown tables — known at parse time)
        // precede resolution-level unknown-key warnings, which follow file
        // order. The order is deterministic; the status page renders it.
        assert_eq!(
            kinds,
            [
                WarningKind::UnknownTable,
                WarningKind::UnknownKey,
                WarningKind::UnknownKey
            ]
        );
        assert_eq!(
            warnings.iter().next().expect("one").path.as_str(),
            "no.such.table"
        );
        let key_paths: Vec<_> = warnings
            .iter()
            .filter(|w| w.kind == WarningKind::UnknownKey)
            .map(|w| w.path.as_str())
            .collect();
        assert_eq!(
            key_paths,
            ["sampling.decibel_window_s", "no.such.table.key"]
        );
    }

    #[test]
    fn unknown_nvs_keys_are_never_consulted() {
        // apply_overrides walks KNOWN_PATHS only — extra NVS entries are
        // simply not read, so they cannot warn or apply.
        let mut nvs = MemorySource::new();
        assert!(nvs.insert("sampling.report_interval_s", "20"));
        assert!(nvs.insert("secret.stuff", "zzz"));
        let (config, warnings) = resolve(Some(DEFAULT_TOML), &nvs).expect("resolves");
        assert_eq!(config.sampling.report_interval_s, 20);
        assert_eq!(warnings.iter().count(), 0);
    }

    #[test]
    fn bad_nvs_override_warns_and_keeps_the_lower_layer() {
        let mut nvs = MemorySource::new();
        assert!(nvs.insert("sampling.decibel_window_ms", "999")); // not a 125 multiple
        assert!(nvs.insert("transport.mqtt.port", "70000")); // out of u16 range
        let (config, warnings) = resolve(Some(DEFAULT_TOML), &nvs).expect("resolves");
        assert_eq!(config.sampling.decibel_window_ms, 1_000);
        assert_eq!(config.transport.mqtt.port, 8883);
        let kinds: Vec<_> = warnings.iter().map(|w| w.kind).collect();
        assert_eq!(kinds, [WarningKind::BadOverride, WarningKind::BadOverride]);
    }

    #[test]
    fn unparseable_nvs_value_warns_and_keeps_the_lower_layer() {
        let mut nvs = MemorySource::new();
        assert!(nvs.insert("sampling.report_interval_s", "thirty"));
        let (config, warnings) = resolve(Some(DEFAULT_TOML), &nvs).expect("resolves");
        assert_eq!(config.sampling.report_interval_s, 60);
        assert_eq!(
            warnings.iter().next().expect("warned").kind,
            WarningKind::BadOverride
        );
    }

    #[test]
    fn wrong_typed_file_value_is_a_hard_error() {
        // The committed file is reviewed code: a bad known value aborts.
        let text = "[sampling]\nreport_interval_s = \"many\"\n";
        let nvs = MemorySource::new();
        let mut t = scratch();
        let err = Config::resolve_in(Some(text), &nvs, &mut t).expect_err("must fail");
        assert!(matches!(
            err,
            ConfigError::BadValue {
                path: "sampling.report_interval_s"
            }
        ));
    }

    #[test]
    fn non_multiple_of_125_window_is_rejected() {
        // decibel.md: Fast windows (125 ms) fold inside the aggregate
        // window — a non-multiple cannot be honored.
        let text = "[sampling]\ndecibel_window_ms = 999\n";
        let nvs = MemorySource::new();
        let mut t = scratch();
        let err = Config::resolve_in(Some(text), &nvs, &mut t).expect_err("must fail");
        assert!(matches!(
            err,
            ConfigError::BadValue {
                path: "sampling.decibel_window_ms"
            }
        ));
    }
}

// ---------------------------------------------------------------------------
// Secret exclusion — secrets are NVS-only by construction
// ---------------------------------------------------------------------------

mod secret_exclusion {
    use super::*;

    #[test]
    fn secret_keys_never_become_config_fields() {
        // The structural guarantee behind the debug-assert tripwire in
        // set_field: the key registry and the schema's paths are disjoint.
        for key in SECRET_KEYS {
            assert!(!KNOWN_PATHS.contains(&key), "{key} must never gain a field");
        }
    }

    #[test]
    fn secret_keys_in_the_file_never_reach_a_config() {
        // A dotted pair key (`wifi.ssid = ...`) is outside the supported
        // TOML subset — it fails the parse and nothing reaches a Config.
        let mut text = String::new();
        for key in SECRET_KEYS {
            text.push_str(&format!("{key} = \"x\"\n"));
        }
        let nvs = MemorySource::new();
        let mut t = scratch();
        let err =
            Config::resolve_in(Some(&text), &nvs, &mut t).expect_err("must not parse");
        assert!(matches!(err, ConfigError::Parse { kind: _, line: 1 }));
    }

    #[test]
    fn nvs_secret_values_never_reach_a_config_or_its_render() {
        // The real provisioning shape: every secret key set in the source
        // (as a provisioned device would be), plus one ordinary override.
        // The config must resolve clean, apply the ordinary override, and
        // render without any secret material.
        let mut nvs = MemorySource::new();
        for (i, key) in SECRET_KEYS.iter().enumerate() {
            assert!(nvs.insert(key, &format!("SECRET-VALUE-{i}")));
        }
        assert!(nvs.insert("sampling.report_interval_s", "30"));
        let (config, warnings) = resolve(Some(DEFAULT_TOML), &nvs).expect("resolves");
        assert_eq!(config.sampling.report_interval_s, 30);
        assert_eq!(warnings.iter().count(), 0);

        let mut out = [0u8; RENDER_CAP];
        let n = config.write_toml(&mut out).expect("fits");
        let rendered = core::str::from_utf8(&out[..n]).expect("utf8");
        for (i, key) in SECRET_KEYS.iter().enumerate() {
            assert!(
                !text_leaks(&rendered, key, i),
                "{key} leaked into the render"
            );
        }
    }

    fn text_leaks(rendered: &str, key: &str, i: usize) -> bool {
        rendered.contains(key) || rendered.contains(&format!("SECRET-VALUE-{i}"))
    }
}

// ---------------------------------------------------------------------------
// Round-trip: resolve → render → re-resolve is the identity
// ---------------------------------------------------------------------------

mod round_trip {
    use super::*;

    #[test]
    fn committed_defaults_round_trip() {
        let (config, warnings) =
            resolve(Some(DEFAULT_TOML), &MemorySource::EMPTY).expect("resolves");
        assert_eq!(warnings.iter().count(), 0);

        let mut out = [0u8; RENDER_CAP];
        let n = config.write_toml(&mut out).expect("fits");
        let rendered = core::str::from_utf8(&out[..n]).expect("utf8");

        let mut t = scratch();
        let mut nvs = MemorySource::new();
        let (again, warnings2) =
            Config::resolve_in(Some(rendered), &nvs, &mut t).expect("re-parses");
        assert_eq!(
            warnings2.iter().count(),
            0,
            "rendered form is clean:\n{rendered}"
        );
        assert_eq!(config, again);
    }

    #[test]
    fn overridden_config_round_trips() {
        let mut nvs = MemorySource::new();
        assert!(nvs.insert("sampling.report_interval_s", "30"));
        assert!(nvs.insert("decibel.calibration_offset_db", "-1.5"));
        assert!(nvs.insert("transport.active", "mqtt"));
        let (config, _) = resolve(Some(DEFAULT_TOML), &nvs).expect("resolves");

        let mut out = [0u8; RENDER_CAP];
        let n = config.write_toml(&mut out).expect("fits");
        let rendered = core::str::from_utf8(&out[..n]).expect("utf8");

        let mut t = scratch();
        let empty = MemorySource::new();
        let (again, _) =
            Config::resolve_in(Some(rendered), &empty, &mut t).expect("re-parses");
        assert_eq!(config, again);
    }

    #[test]
    fn the_longest_legal_config_still_fits_the_render_buffer() {
        let mut nvs = MemorySource::new();
        let long_endpoint = format!("\"https://{}\"", "x".repeat(MAX_VALUE_LEN - 10));
        assert!(nvs.insert("transport.http_endpoint", &long_endpoint));
        let (config, warnings) = resolve(Some(DEFAULT_TOML), &nvs).expect("resolves");
        assert_eq!(warnings.iter().count(), 0);
        let mut out = [0u8; RENDER_CAP];
        let n = config
            .write_toml(&mut out)
            .expect("longest config still fits");
        assert!(n < RENDER_CAP);
    }

    #[test]
    fn known_tables_carry_every_known_path() {
        // Every path's dotted table prefix is a KNOWN_TABLES entry — the
        // parser's table gate and the schema stay in step.
        for path in KNOWN_PATHS {
            let table = path.rsplit_once('.').expect("dotted").0;
            assert!(
                KNOWN_TABLES.contains(&table),
                "{path}'s table {table} is missing from KNOWN_TABLES"
            );
        }
    }
}
