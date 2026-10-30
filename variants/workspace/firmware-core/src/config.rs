//! Runtime configuration shape — the struct behind `config/default.toml`.
//!
//! The config-model task (V2-B2) wires TOML parsing and NVS overrides on top
//! of these defaults; the scaffold pins the shape and the built-in values
//! from the approved spec. Secrets (Wi-Fi credentials, API key, client
//! certificate) never live in this struct — they are read from NVS at
//! runtime only, and nothing here derives a serialisation.

use heapless::String;

/// Which compiled transport publishes measurements.
///
/// A transport the binary was not built with is a config error the boot path
/// must reject — enabling a capability that was not compiled in is the one
/// thing a config edit cannot do (the cargo-flag half of the flag framework).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Http,
    Mqtt,
}

/// Cadences and buffer sizing (spec `config/default.toml` `[sampling]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplingConfig {
    /// The decibel report window; the Fast windows inside it are 125 ms.
    pub decibel_window_ms: u32,
    /// Per-type reporting cadence.
    pub report_interval_s: u32,
    /// Measurements held while disconnected.
    pub offline_buffer: u16,
}

/// Decibel tuning (spec `config/default.toml` `[decibel]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecibelConfig {
    pub overload_threshold_db: f32,
    /// Per-unit calibration value from NVS at runtime; 0.0 until calibrated.
    pub calibration_offset_db: f32,
}

/// PIR front-end (spec `config/default.toml` `[sensors.pir]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PirConfig {
    pub enabled: bool,
    /// Config-chosen pin: no PIR hardware is documented in the source repo,
    /// so the pin is a documented convention, not a board fact.
    pub gpio_pin: u8,
}

/// BME280 (spec `config/default.toml` `[sensors.bme280]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bme280Config {
    pub enabled: bool,
    pub i2c_address: u8,
}

/// Per-sensor enablement and addressing. A sensor that is compiled in but
/// disabled here is absent from the registry — both halves of the flag
/// framework must agree before a driver runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorConfigs {
    /// Decibel is the default-on sensor: the mic is on-board and the default
    /// feature set matches what the server can receive today.
    pub decibel_enabled: bool,
    pub pax: PirConfig,
    pub bme280: Bme280Config,
    pub sht4x_enabled: bool,
    pub scd4x_enabled: bool,
    pub pms5003_enabled: bool,
    pub bh1750_enabled: bool,
}

/// Transport selection and endpoints (spec `config/default.toml`
/// `[transport]` / `[transport.mqtt]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportConfig {
    pub active: TransportKind,
    pub http_endpoint: String<128>,
    pub mqtt_broker: String<64>,
    /// TLS listener; plain 1883 only without mTLS (spec default.toml).
    pub mqtt_port: u16,
}

/// The full runtime configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Device identity. The all-zero placeholder is what a fresh flash boots
    /// with; the real UUID lives in NVS.
    pub sensor_id: String<36>,
    pub sampling: SamplingConfig,
    pub sensors: SensorConfigs,
    pub transport: TransportConfig,
    pub decibel: DecibelConfig,
}

impl Default for Config {
    /// Values from the approved spec's `config/default.toml`.
    fn default() -> Self {
        Self {
            sensor_id: String::try_from("00000000-0000-0000-0000-000000000000").unwrap(),
            sampling: SamplingConfig {
                decibel_window_ms: 1000,
                report_interval_s: 60,
                offline_buffer: 64,
            },
            sensors: SensorConfigs {
                decibel_enabled: true,
                pax: PirConfig {
                    enabled: false,
                    gpio_pin: 4,
                },
                bme280: Bme280Config {
                    enabled: false,
                    i2c_address: 0x76,
                },
                sht4x_enabled: false,
                scd4x_enabled: false,
                pms5003_enabled: false,
                bh1750_enabled: false,
            },
            transport: TransportConfig {
                active: TransportKind::Http,
                http_endpoint: String::try_from("https://ingest.clubins.de/v1/sensors")
                    .unwrap(),
                mqtt_broker: String::try_from("mqtt.clubins.de").unwrap(),
                mqtt_port: 8883,
            },
            decibel: DecibelConfig {
                overload_threshold_db: 120.0,
                calibration_offset_db: 0.0,
            },
        }
    }
}

impl Config {
    /// Every sensor enabled — for `--all-features` device profiles and tests.
    pub fn all_sensors_enabled() -> Self {
        let mut config = Self::default();
        config.sensors.decibel_enabled = true;
        config.sensors.pax.enabled = true;
        config.sensors.bme280.enabled = true;
        config.sensors.sht4x_enabled = true;
        config.sensors.scd4x_enabled = true;
        config.sensors.pms5003_enabled = true;
        config.sensors.bh1750_enabled = true;
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec_config_file() {
        let config = Config::default();
        assert_eq!(
            config.sensor_id.as_str(),
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(config.sampling.decibel_window_ms, 1000);
        assert_eq!(config.sampling.report_interval_s, 60);
        assert_eq!(config.sampling.offline_buffer, 64);
        assert_eq!(config.sensors.bme280.i2c_address, 0x76);
        assert_eq!(config.sensors.pax.gpio_pin, 4);
        assert_eq!(config.transport.active, TransportKind::Http);
        assert_eq!(
            config.transport.http_endpoint.as_str(),
            "https://ingest.clubins.de/v1/sensors"
        );
        assert_eq!(config.transport.mqtt_broker.as_str(), "mqtt.clubins.de");
        assert_eq!(config.transport.mqtt_port, 8883);
        assert_eq!(config.decibel.overload_threshold_db, 120.0);
        assert_eq!(config.decibel.calibration_offset_db, 0.0);
    }

    #[test]
    fn default_enables_only_the_default_sensor() {
        let config = Config::default();
        assert!(config.sensors.decibel_enabled);
        assert!(!config.sensors.pax.enabled);
        assert!(!config.sensors.bme280.enabled);
        assert!(!config.sensors.sht4x_enabled);
        assert!(!config.sensors.scd4x_enabled);
        assert!(!config.sensors.pms5003_enabled);
        assert!(!config.sensors.bh1750_enabled);
    }

    #[test]
    fn all_sensors_enabled_flips_every_sensor() {
        let config = Config::all_sensors_enabled();
        assert!(config.sensors.decibel_enabled);
        assert!(config.sensors.pax.enabled);
        assert!(config.sensors.bme280.enabled);
        assert!(config.sensors.sht4x_enabled);
        assert!(config.sensors.scd4x_enabled);
        assert!(config.sensors.pms5003_enabled);
        assert!(config.sensors.bh1750_enabled);
    }
}
