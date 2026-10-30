//! The firmware binary: a slim composition root.
//!
//! Config in, compiled-and-enabled sensors + one transport out, sampling
//! loop runs. All capability logic lives in the workspace crates; this file
//! only wires them together, and the wiring arms are the cargo feature
//! flags (spec §Feature Matrix). The device personality exists only for the
//! Xtensa target; host builds compile the composition plus the test suite.

#![cfg_attr(target_arch = "xtensa", no_std)]
#![cfg_attr(target_arch = "xtensa", no_main)]

// The device personality allocates (registry boxes, later Wi-Fi buffers);
// esp-alloc supplies the global allocator, esp-backtrace the panic handler
// (halt both cores — a firmware panic must not continue on the other core).
#[cfg(target_arch = "xtensa")]
extern crate alloc;
#[cfg(target_arch = "xtensa")]
use alloc::boxed::Box;

#[cfg(target_arch = "xtensa")]
mod device {
    use firmware_core::config::Config;

    use esp_backtrace as _;

    #[esp_hal::main]
    fn main() -> ! {
        // 64 KiB DRAM heap. The macro adds a heap region to the global
        // allocator registered by esp-alloc's `global-allocator` feature;
        // it must run before the first allocation, and the macro expands
        // in statement position (it declares a local static), so it lives
        // here and not at module level.
        esp_alloc::heap_allocator!(size: 64 * 1024);
        // HAL init: clocks and peripheral access. Bus wiring (I2S/I2C/UART),
        // Wi-Fi bring-up and the watchdog feed land with the bus and
        // transport tasks — this loop is the composition skeleton they
        // plug into.
        let _peripherals = esp_hal::init(esp_hal::Config::default());

        let config = Config::default();
        let mut registry = super::registry_from_features(&config);

        loop {
            // Sample everything compiled and enabled; each measurement goes
            // to the transport publish path, which lands with the transport
            // tasks.
            registry.sample_all(|measurement| {
                let _ = measurement.ingest_type();
            });
        }
    }
}

#[cfg(not(target_arch = "xtensa"))]
fn main() {
    // Host build: the composition compiles, but there is nothing to run —
    // device behaviour is exercised by the test suite and by the CI link
    // job for the Xtensa target.
    println!("firmware: host compose-only build (device code targets xtensa)");
}

/// Registry composition for the device and the host tests: every compiled
/// driver arm, gated on its config enablement. A capability that is
/// compiled in but disabled in config is absent from the registry — both
/// halves of the flag framework must agree before a driver runs.
#[cfg(any(test, target_arch = "xtensa"))]
fn registry_from_features(
    #[allow(unused_variables)] config: &firmware_core::config::Config,
) -> firmware_core::registry::Registry {
    let mut registry = firmware_core::registry::Registry::new();

    #[cfg(feature = "sensor-decibel")]
    {
        if config.sensors.decibel_enabled {
            registry
                .push(Box::new(driver_decibel::DecibelSensor::new(
                    &config.decibel,
                )))
                .expect("registry sizing");
        }
    }

    #[cfg(feature = "sensor-pax")]
    {
        if config.sensors.pax.enabled {
            registry
                .push(Box::new(driver_pax::PirSensor::new(
                    config.sensors.pax.gpio_pin,
                )))
                .expect("registry sizing");
        }
    }

    #[cfg(feature = "sensor-bme280")]
    {
        if config.sensors.bme280.enabled {
            registry
                .push(Box::new(driver_bme280::Bme280Driver::new(
                    &config.sensors.bme280,
                )))
                .expect("registry sizing");
        }
    }

    #[cfg(feature = "sensor-sht4x")]
    {
        if config.sensors.sht4x_enabled {
            registry
                .push(Box::new(driver_sht4x::Sht4xDriver::new()))
                .expect("registry sizing");
        }
    }

    #[cfg(feature = "sensor-scd4x")]
    {
        if config.sensors.scd4x_enabled {
            registry
                .push(Box::new(driver_scd4x::Scd4xDriver::new()))
                .expect("registry sizing");
        }
    }

    #[cfg(feature = "sensor-pms5003")]
    {
        if config.sensors.pms5003_enabled {
            registry
                .push(Box::new(driver_pms5003::Pms5003Driver::new()))
                .expect("registry sizing");
        }
    }

    #[cfg(feature = "sensor-bh1750")]
    {
        if config.sensors.bh1750_enabled {
            registry
                .push(Box::new(driver_bh1750::Bh1750Driver::new()))
                .expect("registry sizing");
        }
    }

    registry
}

#[cfg(test)]
mod tests {
    use firmware_core::config::Config;

    /// The compiled sensor count for the feature set under test — the
    /// expected registry size when every sensor is config-enabled.
    fn compiled_sensor_count() -> usize {
        let mut count = 0;
        #[cfg(feature = "sensor-decibel")]
        {
            count += 1;
        }
        #[cfg(feature = "sensor-pax")]
        {
            count += 1;
        }
        #[cfg(feature = "sensor-bme280")]
        {
            count += 1;
        }
        #[cfg(feature = "sensor-sht4x")]
        {
            count += 1;
        }
        #[cfg(feature = "sensor-scd4x")]
        {
            count += 1;
        }
        #[cfg(feature = "sensor-pms5003")]
        {
            count += 1;
        }
        #[cfg(feature = "sensor-bh1750")]
        {
            count += 1;
        }
        count
    }

    /// The compiled feature names, for the boot report. One row per spec
    /// flag; `cfg!` keeps the table honest with the Cargo.toml feature list.
    fn compiled_feature_names() -> Vec<&'static str> {
        [
            ("sensor-decibel", cfg!(feature = "sensor-decibel")),
            ("sensor-pax", cfg!(feature = "sensor-pax")),
            ("sensor-bme280", cfg!(feature = "sensor-bme280")),
            ("sensor-sht4x", cfg!(feature = "sensor-sht4x")),
            ("sensor-scd4x", cfg!(feature = "sensor-scd4x")),
            ("sensor-pms5003", cfg!(feature = "sensor-pms5003")),
            ("sensor-bh1750", cfg!(feature = "sensor-bh1750")),
            ("transport-http", cfg!(feature = "transport-http")),
            ("transport-mqtt", cfg!(feature = "transport-mqtt")),
            ("transport-mtls", cfg!(feature = "transport-mtls")),
            ("ota", cfg!(feature = "ota")),
            ("provisioning", cfg!(feature = "provisioning")),
            ("deep-sleep", cfg!(feature = "deep-sleep")),
            ("logging-defmt", cfg!(feature = "logging-defmt")),
            ("ops-status", cfg!(feature = "ops-status")),
        ]
        .into_iter()
        .filter(|(_, compiled)| *compiled)
        .map(|(name, _)| name)
        .collect()
    }

    #[test]
    fn registry_reports_exactly_the_compiled_sensors() {
        let config = Config::all_sensors_enabled();
        let registry = super::registry_from_features(&config);
        assert_eq!(
            registry.len(),
            compiled_sensor_count(),
            "registry must hold one entry per compiled driver"
        );
    }

    #[test]
    fn disabled_sensors_are_absent_not_idle() {
        // Default config enables only decibel; the registry must reflect
        // the config, not the compiled feature set.
        let config = Config::default();
        let registry = super::registry_from_features(&config);
        let expected = usize::from(
            cfg!(feature = "sensor-decibel") && config.sensors.decibel_enabled,
        );
        assert_eq!(registry.len(), expected);
    }

    #[test]
    fn transport_mtls_implies_transport_mqtt() {
        // Declared as a feature dependency in Cargo.toml; asserted here so
        // the implication is tested wherever the matrix runs. The flags are
        // read into locals so the assert is not constant-folded (clippy
        // assertions_on_constants).
        let mtls_compiled = cfg!(feature = "transport-mtls");
        let mqtt_compiled = cfg!(feature = "transport-mqtt");
        assert!(!mtls_compiled || mqtt_compiled);
    }

    #[test]
    fn compiled_feature_names_cover_the_flag_vocabulary() {
        for name in compiled_feature_names() {
            assert!(
                [
                    "sensor-decibel",
                    "sensor-pax",
                    "sensor-bme280",
                    "sensor-sht4x",
                    "sensor-scd4x",
                    "sensor-pms5003",
                    "sensor-bh1750",
                    "transport-http",
                    "transport-mqtt",
                    "transport-mtls",
                    "ota",
                    "provisioning",
                    "deep-sleep",
                    "logging-defmt",
                    "ops-status",
                ]
                .contains(&name),
                "{name} is not a spec flag name"
            );
        }
    }

    #[test]
    #[cfg(feature = "ops-status")]
    fn boot_report_lists_every_compiled_feature() {
        let features = compiled_feature_names();
        let report = ops_status::boot_report(&features);
        for feature in &features {
            assert!(
                report.message.contains(feature),
                "boot report missing {feature}"
            );
        }
    }
}
