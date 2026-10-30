//! Host tests — layered runtime configuration (spec §Configuration).
//!
//! One test per VERIFIED BY behaviour: precedence order, unknown-key
//! tolerance (warn list), secret exclusion, round-trip parse — plus the
//! committed-file contract (completeness, builtin↔file consistency) and
//! the tolerance fallbacks that keep a bad layer from bricking a boot.

use firmware_v5_hsm::config::{
    Config, ConfigWarning, DEFAULT_TOML, FORBIDDEN_SECRET_NAMES, PartialConfig,
    SensorKind, TUNABLE_PATHS, TomlTable, TomlValue, TransportKind, config_key_paths,
    parse_toml,
};

/// The committed `config/default.toml` as a layer; the file must parse
/// clean in CI — a warning here means the committed file regressed.
fn file_layer() -> PartialConfig {
    let layer = PartialConfig::from_toml(DEFAULT_TOML);
    assert!(
        layer.warnings.is_empty(),
        "committed default.toml must parse clean: {:?}",
        layer.warnings
    );
    layer
}

/// builtin < default.toml: the committed file applied over built-ins.
fn file_layered() -> (Config, Vec<ConfigWarning>) {
    let file = file_layer();
    PartialConfig::from_layers(Config::builtin(), &[&file])
}

/// builtin < default.toml < nvs.
fn nvs_layered(nvs: &PartialConfig) -> (Config, Vec<ConfigWarning>) {
    let file = file_layer();
    PartialConfig::from_layers(Config::builtin(), &[&file, nvs])
}

// --- precedence order -----------------------------------------------------

#[test]
fn nvs_overrides_beat_the_file_which_beats_builtins() {
    let mut nvs = PartialConfig::default();
    nvs.sampling.interval_ms = Some(2000);
    nvs.publish.offline_buffer = Some(16);
    nvs.transport.active = Some(TransportKind::Mqtt);
    nvs.sensors.decibel.calibration_offset_db = Some(-2.5);

    let (cfg, warnings) = nvs_layered(&nvs);
    assert_eq!(warnings, vec![], "all NVS values are valid");
    // NVS values win on every key it sets:
    assert_eq!(cfg.sampling.interval_ms, 2000);
    assert_eq!(cfg.publish.offline_buffer, 16);
    assert_eq!(cfg.transport.active, TransportKind::Mqtt);
    assert_eq!(cfg.sensors.decibel.calibration_offset_db, -2.5);
    // Untouched keys keep the file layer's values:
    assert_eq!(cfg.publish.interval_ms, 15_000);
    assert_eq!(cfg.transport.mqtt.port, 8883);
}

#[test]
fn an_empty_nvs_layer_changes_nothing() {
    let (with_nvs, _) = nvs_layered(&PartialConfig::default());
    let (without, _) = file_layered();
    assert_eq!(with_nvs, without);
}

#[test]
fn the_sensor_interval_falls_back_to_sampling_when_unset() {
    // Resolver contract: a sensor with no cadence of its own uses the
    // `[sampling]` default. (The committed file pins every cadence
    // explicitly, so this is observable on a directly-built config.)
    let mut cfg = Config::builtin();
    cfg.sampling.interval_ms = 4000;
    cfg.sensors.sht4x.interval_ms = None;
    assert_eq!(cfg.sample_interval_ms(SensorKind::Sht4x), 4000);
    // An explicitly-set cadence still wins:
    cfg.sensors.sht4x.interval_ms = Some(9000);
    assert_eq!(cfg.sample_interval_ms(SensorKind::Sht4x), 9000);
}

#[test]
fn a_corrupt_higher_layer_falls_back_to_the_layer_below() {
    let layer = PartialConfig::from_toml("no equals sign here\n");
    assert_eq!(layer.bound_keys(), 0);
    assert_eq!(layer.warnings.len(), 1);
    assert!(matches!(layer.warnings[0], ConfigWarning::LayerSyntax(_)));

    let file = file_layer();
    let (cfg, warnings) = PartialConfig::from_layers(Config::builtin(), &[&file, &layer]);
    assert_eq!(cfg, Config::builtin(), "dropped layer changes nothing");
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, ConfigWarning::LayerSyntax(_)))
    );
}

// --- committed-file contract ----------------------------------------------

#[test]
fn the_committed_file_binds_every_documented_tunable() {
    let layer = file_layer();
    assert_eq!(
        layer.bound_keys(),
        TUNABLE_PATHS.len(),
        "config/default.toml must cover exactly the documented tunables"
    );
}

#[test]
fn the_builtin_layer_matches_the_committed_file() {
    // The compiled-in last-resort layer must mirror the committed file —
    // applying the file layer over built-ins is a no-op.
    let (cfg, warnings) = file_layered();
    assert!(warnings.is_empty());
    assert_eq!(cfg, Config::builtin());
}

// --- unknown-key tolerance (warn list) ------------------------------------

#[test]
fn unknown_keys_are_tolerated_and_listed() {
    let src = "\n# A newer firmware wrote these.\nrobot_future = true\n\n[sampling]\ninterval_ms = 2500\nquantum_flux = 7\n\n[transport.mqtt]\nhost = \"broker\"\nbogus = 1\n";
    let layer = PartialConfig::from_toml(src);
    let warnings = &layer.warnings;
    // Known keys still bind:
    assert_eq!(layer.sampling.interval_ms, Some(2500));
    assert_eq!(layer.transport.mqtt.host.as_deref(), Some("broker"));
    // Unknown keys land on the warn list with their dotted paths:
    for key in [
        "robot_future",
        "sampling.quantum_flux",
        "transport.mqtt.bogus",
    ] {
        assert!(
            warnings.contains(&ConfigWarning::UnknownKey(String::from(key))),
            "warn list must contain `{key}`: {warnings:?}"
        );
    }
    assert_eq!(
        warnings.len(),
        3,
        "no other warnings expected: {warnings:?}"
    );
}

#[test]
fn bad_values_warn_and_keep_the_previous_value() {
    let mut nvs = PartialConfig::default();
    nvs.sampling.interval_ms = Some(0); // interval floor
    nvs.device.sensor_id = Some(String::from("not-a-uuid"));
    nvs.sensors.decibel.window_ms = Some(5); // below the 1 s window floor
    nvs.sensors.decibel.calibration_offset_db = Some(99.0); // beyond ±40 dB
    nvs.publish.backoff_initial_ms = Some(10_000);
    nvs.publish.backoff_max_ms = Some(1_000); // max below initial
    nvs.sensors.pax.pir_pin = Some(255); // out of the pin range

    let (cfg, warnings) = nvs_layered(&nvs);
    // Each rejected value keeps the layer-below value:
    assert_eq!(cfg.sampling.interval_ms, 1000);
    assert_eq!(cfg.device.sensor_id, "00000000-0000-0000-0000-000000000000");
    assert_eq!(cfg.sensors.decibel.window_ms, 60_000);
    assert_eq!(cfg.sensors.decibel.calibration_offset_db, 0.0);
    assert_eq!(cfg.publish.backoff_initial_ms, 500);
    assert_eq!(cfg.publish.backoff_max_ms, 30_000);
    assert_eq!(cfg.sensors.pax.pir_pin, 4);
    // …and each rejection is named on the warn list:
    let warned_keys: Vec<&str> = warnings
        .iter()
        .filter_map(|w| match w {
            ConfigWarning::BadValue { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect();
    for key in [
        "sampling.interval_ms",
        "device.sensor_id",
        "sensors.decibel.window_ms",
        "sensors.decibel.calibration_offset_db",
        "publish.backoff_max_ms",
        "sensors.pax.pir_pin",
    ] {
        assert!(
            warned_keys.contains(&key),
            "warn list must contain `{key}`: {warnings:?}"
        );
    }
    assert_eq!(
        warned_keys.len(),
        6,
        "backoff_initial_ms itself is valid: {warnings:?}"
    );
}

#[test]
fn warnings_read_as_human_text() {
    let warning = ConfigWarning::UnknownKey(String::from("sampling.quantum_flux"));
    assert_eq!(
        warning.to_string(),
        "unknown config key `sampling.quantum_flux` — ignored"
    );
}

// --- secret exclusion ------------------------------------------------------

fn secret_check(doc: &TomlTable, what: &str) {
    for path in config_key_paths(doc) {
        let lower = path.to_ascii_lowercase();
        for name in FORBIDDEN_SECRET_NAMES {
            assert!(
                !lower.split('.').any(|seg| seg.contains(name)),
                "{what} must never contain secret field `{name}` (key `{path}`)"
            );
        }
    }
}

#[test]
fn serialized_config_names_no_secret_field() {
    let (cfg, _) = file_layered();
    let serialized = cfg.to_toml();
    let doc = parse_toml(&serialized).expect("serializer output must parse");
    secret_check(&doc, "serialized config");
}

#[test]
fn the_committed_file_carries_no_secret_keys() {
    let doc = parse_toml(DEFAULT_TOML).expect("committed default.toml must parse");
    secret_check(&doc, "config/default.toml");
}

#[test]
fn the_secret_schema_has_no_file_layer_at_all() {
    // Belt to the suspenders: an NVS secret blob shaped like a config
    // layer must still bind nothing — the Partial* structs have no
    // credential fields, so the whole blob can only warn.
    let src = "\n[device]\nsensor_id = \"00000000-0000-0000-0000-000000000000\"\n\n[transport.http]\nendpoint = \"https://ingest.clubins.de\"\n";
    let layer = PartialConfig::from_toml(src);
    assert_eq!(
        layer.device.sensor_id.as_deref(),
        Some("00000000-0000-0000-0000-000000000000")
    );
    assert!(layer.warnings.is_empty());
    // The layer cannot even *express* a secret: only schema fields exist.
    let json = format!("{layer:?}");
    for name in FORBIDDEN_SECRET_NAMES {
        assert!(!json.to_ascii_lowercase().contains(name));
    }
}

// --- round-trip parse -------------------------------------------------------

#[test]
fn serializing_config_round_trips_through_the_parser() {
    let (cfg, _) = file_layered();
    let layer = PartialConfig::from_toml(&cfg.to_toml());
    assert!(
        layer.warnings.is_empty(),
        "the serializer's own output must parse clean"
    );
    let (reapplied, warnings) = PartialConfig::from_layers(Config::builtin(), &[&layer]);
    assert!(warnings.is_empty());
    assert_eq!(
        reapplied, cfg,
        "TOML round-trip must preserve every tunable"
    );
}

#[test]
fn round_trip_survives_a_fully_nvs_overridden_config() {
    let mut nvs = PartialConfig::default();
    nvs.sampling.interval_ms = Some(7777);
    nvs.transport.active = Some(TransportKind::Mqtt);
    nvs.transport.mqtt.port = Some(1883);
    nvs.sensors.decibel.calibration_offset_db = Some(-3.75);
    let (cfg, _) = nvs_layered(&nvs);

    let layer = PartialConfig::from_toml(&cfg.to_toml());
    assert!(layer.warnings.is_empty());
    let (reapplied, _) = PartialConfig::from_layers(Config::builtin(), &[&layer]);
    assert_eq!(reapplied, cfg);
}

// --- parser diagnostics -----------------------------------------------------

#[test]
fn the_parser_rejects_the_deliberate_subset_with_line_numbers() {
    for (src, expected) in [
        ("a = [1, 2]", "unsupported: arrays"),
        ("a = \"\"\"x\"\"\"", "unsupported: multi-line strings"),
        ("a = 'literal'", "unsupported: literal strings"),
        ("a = {x = 1}", "unsupported: inline tables"),
        ("a = 0x2A", "unsupported: non-decimal integers"),
        ("a", "syntax: expected `key = value`"),
        ("a =", "syntax: missing value"),
        ("a = tru", "syntax: unrecognized value"),
        ("[unclosed", "syntax: table header must end with `]`"),
        ("a = 1\na = 2", "duplicate key `a`"),
        ("[x]\n[x]", "duplicate table `x`"),
    ] {
        let err = parse_toml(src).expect_err(src);
        assert!(err.to_string().contains(expected), "{src}: {err}");
        assert!(err.line >= 1);
    }
}

#[test]
fn the_parser_tolerates_comments_blank_lines_and_underscores() {
    let doc = parse_toml(
        "# leading comment\n\n[sensors.decibel]\ninterval_ms = 1_000 # mid-line comment\n",
    )
    .unwrap();
    let TomlValue::Table(sensors) = doc.get("sensors").unwrap() else {
        panic!("`sensors` must be a table");
    };
    let TomlValue::Table(decibel) = sensors.get("decibel").unwrap() else {
        panic!("`sensors.decibel` must be a table");
    };
    let TomlValue::Int(n) = decibel.get("interval_ms").unwrap() else {
        panic!("`interval_ms` must be an integer");
    };
    assert_eq!(*n, 1_000, "underscores are stripped from integers");
}
