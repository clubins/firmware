//! Layout sanity: the two named host gates for the flag framework.
//!
//! - `feature_flag_resolution` — the registry must mirror the cargo feature
//!   selection of this build exactly, so a mis-wired flag fails in the combo
//!   that broke it. Runs under every flag-matrix combination.
//! - `module_presence` — every stub module toggled by its flag is addressable
//!   in this build, and the always-on anchor modules exist.

use clubins_firmware::sensor::SensorKind;
use clubins_firmware::{config, registry, state};
use std::collections::HashSet;

/// Feature-flag resolution: the compiled-in sensor set must exactly match the
/// cargo feature selection of this build.
#[test]
fn feature_flag_resolution() {
    let enabled: HashSet<SensorKind> = registry::enabled_sensor_metas()
        .map(|meta| meta.kind)
        .collect();
    let selected: HashSet<SensorKind> = SensorKind::ALL
        .iter()
        .copied()
        .filter(|kind| match kind {
            SensorKind::Decibel => cfg!(feature = "sensor-decibel"),
            SensorKind::Pax => cfg!(feature = "sensor-pax"),
            SensorKind::Bme280 => cfg!(feature = "sensor-bme280"),
            SensorKind::Sht4x => cfg!(feature = "sensor-sht4x"),
            SensorKind::Scd4x => cfg!(feature = "sensor-scd4x"),
            SensorKind::Pms5003 => cfg!(feature = "sensor-pms5003"),
            SensorKind::Bh1750 => cfg!(feature = "sensor-bh1750"),
        })
        .collect();

    assert_eq!(
        enabled, selected,
        "registry resolution must mirror the cargo feature selection"
    );

    for meta in registry::enabled_sensor_metas() {
        assert!(!meta.ingest_type.is_empty());
        assert_eq!(meta.ingest_type, meta.kind.ingest_type());
    }
}

/// Module presence: every stub module toggled by its flag is addressable in
/// this build, and the always-on anchor modules exist.
#[test]
fn module_presence() {
    assert_eq!(config::MODULE, "config");
    assert_eq!(state::MODULE, "state");

    macro_rules! assert_present {
        // `$module` interpolates the whole `…::MODULE` path: a `path`
        // metavariable is opaque, so `::MODULE` cannot be appended after it.
        ($feature:literal, $module:expr, $name:literal) => {
            #[cfg(feature = $feature)]
            {
                assert_eq!($module, $name);
            }
        };
    }

    assert_present!(
        "sensor-decibel",
        clubins_firmware::sensors::pdm_mic::MODULE,
        "sensors::pdm_mic"
    );
    assert_present!(
        "sensor-pax",
        clubins_firmware::sensors::pir::MODULE,
        "sensors::pir"
    );
    assert_present!(
        "sensor-bme280",
        clubins_firmware::sensors::bme280::MODULE,
        "sensors::bme280"
    );
    assert_present!(
        "sensor-sht4x",
        clubins_firmware::sensors::sht4x::MODULE,
        "sensors::sht4x"
    );
    assert_present!(
        "sensor-scd4x",
        clubins_firmware::sensors::scd4x::MODULE,
        "sensors::scd4x"
    );
    assert_present!(
        "sensor-pms5003",
        clubins_firmware::sensors::pms5003::MODULE,
        "sensors::pms5003"
    );
    assert_present!(
        "sensor-bh1750",
        clubins_firmware::sensors::bh1750::MODULE,
        "sensors::bh1750"
    );
    assert_present!(
        "transport-http",
        clubins_firmware::transports::http::MODULE,
        "transports::http"
    );
    assert_present!(
        "transport-mqtt",
        clubins_firmware::transports::mqtt::MODULE,
        "transports::mqtt"
    );
    assert_present!(
        "transport-mtls",
        clubins_firmware::ops::mtls::MODULE,
        "ops::mtls"
    );
    assert_present!("ota", clubins_firmware::ops::ota::MODULE, "ops::ota");
    assert_present!(
        "provisioning",
        clubins_firmware::ops::ble_provisioning::MODULE,
        "ops::ble_provisioning"
    );
    assert_present!(
        "deep-sleep",
        clubins_firmware::ops::deep_sleep::MODULE,
        "ops::deep_sleep"
    );
    assert_present!(
        "logging-defmt",
        clubins_firmware::ops::logging_defmt::MODULE,
        "ops::logging_defmt"
    );
    assert_present!(
        "ops-status",
        clubins_firmware::ops::status::MODULE,
        "ops::status"
    );

    // Anchor types exist and their wire names match the ingest contracts.
    assert_eq!(SensorKind::Decibel.ingest_type(), "decibel");
    assert_eq!(SensorKind::Pax.ingest_type(), "pax");
    assert_eq!(SensorKind::Bme280.ingest_type(), "weather");
    assert_eq!(SensorKind::Sht4x.ingest_type(), "weather");
    assert_eq!(SensorKind::Scd4x.ingest_type(), "co2");
    assert_eq!(SensorKind::Pms5003.ingest_type(), "pm25");
    assert_eq!(SensorKind::Bh1750.ingest_type(), "light");
}
