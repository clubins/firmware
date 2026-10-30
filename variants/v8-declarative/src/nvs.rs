//! The NVS override layer — the top rung of the config precedence ladder
//! (`NVS > config/default.toml > built-in`). Per the spec, NVS is
//! hardware-bound: the flash-backed [`OverrideSource`] implementation
//! lives behind `esp32s3` (Xtensa-only, with the ops/driver layer), while
//! the resolution logic here is pure and host-tested — the spec requires
//! the precedence rule to be tested on the host. Without a device source
//! the config resolves from the committed document alone ([`NoOverrides`]).
//!
//! NVS state is written by provisioning onto devices nobody reviews, so
//! the failure posture here is the opposite of the committed document's:
//! nothing read from NVS aborts the boot. Unknown keys and unusable
//! values are collected as [`config::Warnings`] and the lower-precedence
//! value stands — a broken override degrades to the tested configuration,
//! visibly. Overrides arrive through [`OverrideSource`], so host tests
//! inject plain vectors where the device plugs its flash partition.
//!
//! The [`Namespace`] partitioning is shared vocabulary between the
//! provisioner (writes) and this resolver (reads). The `Wire` namespace
//! holds the secret material (Wi-Fi credentials, the API key, TLS
//! identity); its values are consumed directly by the transport
//! subsystem from NVS and never enter [`config::Config`] — the
//! serializable config view stays secret-free by construction
//! ([`config::SECRET_MARKERS`]).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::config::{self, Config, Warnings};

/// The NVS partitions, shared vocabulary between provisioning (writes)
/// and config resolution (reads).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    /// Device identity: the sensor UUID.
    Identity,
    /// Connection material, including secrets (Wi-Fi credentials, the API
    /// key, client certificate and key). Read by the transport subsystem,
    /// never serialized into `Config`.
    Wire,
    /// Per-unit measurement calibration.
    Cal,
    /// Schema overrides: `section.field` keys mirroring the names in
    /// `config/default.toml` (e.g. `sampling.report_interval_s`).
    Schema,
}

impl Namespace {
    pub fn as_str(self) -> &'static str {
        match self {
            Namespace::Identity => "identity",
            Namespace::Wire => "wire",
            Namespace::Cal => "cal",
            Namespace::Schema => "schema",
        }
    }
}

/// Where overrides come from. The device supplies its flash-backed
/// reader; host tests inject plain vectors ([`VecOverrides`]).
pub trait OverrideSource {
    /// Hand every stored override to `f` as `(namespace, key, value)`.
    /// Keys within a namespace are unique; if a backing store somehow
    /// yields a duplicate, the last one wins.
    fn for_each(&self, f: &mut dyn FnMut(Namespace, &str, &str));
}

/// A plain in-memory override source — the injectable host stand-in for
/// the device's NVS, and the shape integration tests use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VecOverrides(pub Vec<(Namespace, String, String)>);

impl OverrideSource for VecOverrides {
    fn for_each(&self, f: &mut dyn FnMut(Namespace, &str, &str)) {
        for (ns, key, value) in &self.0 {
            f(*ns, key, value);
        }
    }
}

/// The no-op source for devices without NVS-backed overrides: applying it
/// changes nothing and warns about nothing.
pub struct NoOverrides;

impl OverrideSource for NoOverrides {
    fn for_each(&self, _f: &mut dyn FnMut(Namespace, &str, &str)) {}
}

/// Apply NVS overrides onto a resolved config, in place. Every skipped
/// override is explained in `warnings`; the config keeps the value from
/// the rung below.
pub fn apply(config: &mut Config, source: &dyn OverrideSource, warnings: &mut Warnings) {
    source.for_each(&mut |ns, key, value| match ns {
        Namespace::Identity => apply_identity(config, key, value, warnings),
        Namespace::Cal => apply_cal(config, key, value, warnings),
        Namespace::Schema => apply_schema(config, key, value, warnings),
        // Secret material is consumed by the transport subsystem straight
        // from NVS; the config resolver sees (and serializes) none of it.
        Namespace::Wire => warnings.push(format!(
            "nvs: wire override '{key}' is not applied to the device config (transport subsystem reads it directly)"
        )),
    });
}

fn apply_identity(config: &mut Config, key: &str, value: &str, warnings: &mut Warnings) {
    match key {
        "sensor_id" => {
            if config::is_uuid_like(value) {
                config.sensor.id = value.into();
            } else {
                warnings.push(
                    "nvs: identity.sensor_id is not a UUID — keeping the lower-precedence value"
                        .to_string(),
                );
            }
        }
        other => warnings.push(format!("nvs: unknown identity key '{other}' — ignored")),
    }
}

fn apply_cal(config: &mut Config, key: &str, value: &str, warnings: &mut Warnings) {
    match key {
        "decibel_calibration_offset_db" => match value.parse::<f64>() {
            // Same window the committed document enforces for
            // calibration_offset_db — a value rejected there is rejected here.
            Ok(v) if v.is_finite() && (-120.0..=120.0).contains(&v) => {
                config.decibel.calibration_offset_db = v;
            }
            _ => warnings.push(
                "nvs: cal.decibel_calibration_offset_db must be a finite number in -120..=120 — keeping the lower-precedence value".to_string(),
            ),
        },
        other => warnings.push(format!("nvs: unknown cal key '{other}' — ignored")),
    }
}

/// Schema overrides mirror the committed document's key names with string
/// values. Only known keys apply; each is parsed and range-checked with
/// the same bounds `config.rs` enforces, and a failure warns + skips —
/// the default.toml value (itself tested) stands.
fn apply_schema(config: &mut Config, key: &str, value: &str, warnings: &mut Warnings) {
    match key {
        "sampling.decibel_window_ms" => apply_u32(
            config,
            value,
            1,
            u32::MAX,
            warnings,
            |c| &mut c.sampling.decibel_window_ms,
        ),
        "sampling.report_interval_s" => apply_u32(
            config,
            value,
            1,
            u32::MAX,
            warnings,
            |c| &mut c.sampling.report_interval_s,
        ),
        "sampling.offline_buffer" => apply_u32(
            config,
            value,
            0,
            u32::MAX,
            warnings,
            |c| &mut c.sampling.offline_buffer,
        ),
        "sensors.bme280.i2c_address" => {
            apply_u8(config, value, 0, 0x7f, warnings, |c| {
                &mut c.sensors.bme280.i2c_address
            });
        }
        "sensors.pir.gpio_pin" => {
            apply_u8(config, value, 0, 48, warnings, |c| &mut c.sensors.pir.gpio_pin);
        }
        "transport.http_endpoint" => {
            if value.is_empty() {
                warnings.push(
                    "nvs: schema.transport.http_endpoint must not be empty — keeping the lower-precedence value"
                        .to_string(),
                );
            } else {
                config.transport.http_endpoint = value.into();
            }
        }
        "transport.mqtt.broker" => {
            if value.is_empty() {
                warnings.push(
                    "nvs: schema.transport.mqtt.broker must not be empty — keeping the lower-precedence value"
                        .to_string(),
                );
            } else {
                config.transport.mqtt.broker = value.into();
            }
        }
        "transport.mqtt.port" => {
            apply_u16(config, value, 1, 65535, warnings, |c| &mut c.transport.mqtt.port);
        }
        "decibel.overload_threshold_db" => apply_f64(
            config,
            value,
            0.0,
            200.0,
            warnings,
            |c| &mut c.decibel.overload_threshold_db,
        ),
        "decibel.calibration_offset_db" => apply_f64(
            config,
            value,
            -120.0,
            120.0,
            warnings,
            |c| &mut c.decibel.calibration_offset_db,
        ),
        other => warnings.push(format!(
            "nvs: unknown schema key '{other}' — ignored, the lower-precedence value stands"
        )),
    }
}

fn apply_u32(
    config: &mut Config,
    raw: &str,
    min: u32,
    max: u32,
    warnings: &mut Warnings,
    pick: fn(&mut Config) -> &mut u32,
) {
    match raw.parse::<u32>() {
        Ok(v) if (min..=max).contains(&v) => *pick(config) = v,
        _ => warnings.push(format!(
            "nvs: schema override '{raw}' is not a usable integer in {min}..={max} — keeping the lower-precedence value"
        )),
    }
}

fn apply_u16(
    config: &mut Config,
    raw: &str,
    min: u16,
    max: u16,
    warnings: &mut Warnings,
    pick: fn(&mut Config) -> &mut u16,
) {
    match raw.parse::<u16>() {
        Ok(v) if (min..=max).contains(&v) => *pick(config) = v,
        _ => warnings.push(format!(
            "nvs: schema override '{raw}' is not a usable integer in {min}..={max} — keeping the lower-precedence value"
        )),
    }
}

fn apply_u8(
    config: &mut Config,
    raw: &str,
    min: u8,
    max: u8,
    warnings: &mut Warnings,
    pick: fn(&mut Config) -> &mut u8,
) {
    match raw.parse::<u8>() {
        Ok(v) if (min..=max).contains(&v) => *pick(config) = v,
        _ => warnings.push(format!(
            "nvs: schema override '{raw}' is not a usable integer in {min}..={max} — keeping the lower-precedence value"
        )),
    }
}

fn apply_f64(
    config: &mut Config,
    raw: &str,
    min: f64,
    max: f64,
    warnings: &mut Warnings,
    pick: fn(&mut Config) -> &mut f64,
) {
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() && (min..=max).contains(&v) => *pick(config) = v,
        _ => warnings.push(format!(
            "nvs: schema override '{raw}' is not a usable number in {min}..={max} — keeping the lower-precedence value"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TransportKind;

    fn resolved(src: &str) -> (Config, Warnings) {
        let parsed = Config::parse(src).expect("document parses");
        (parsed.config, parsed.warnings)
    }

    fn overridden(config: &mut Config, overrides: impl OverrideSource) -> Warnings {
        let mut warnings = Warnings::default();
        apply(config, &overrides, &mut warnings);
        warnings
    }

    #[test]
    fn identity_override_beats_document_and_builtin() {
        let (mut config, _) = resolved(
            r#"
            [sensor]
            id = "0b1ac1e7-38f8-4f3e-9d0a-8b6a2c4e5f70"
            "#,
        );
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![(
                Namespace::Identity,
                "sensor_id".into(),
                "11111111-2222-4333-8444-555555555555".into(),
            )]),
        );
        assert!(warnings.is_empty());
        assert_eq!(config.sensor.id, "11111111-2222-4333-8444-555555555555");
    }

    #[test]
    fn full_ladder_builtin_below_document_below_nvs() {
        // Built-in rung: no document, no overrides.
        let mut config = Config::builtin();
        assert_eq!(config.sampling.report_interval_s, 60);

        // Document rung: default.toml beats the built-in.
        let (mut config, _) = resolved(
            r#"
            [sampling]
            report_interval_s = 90
            "#,
        );
        assert_eq!(config.sampling.report_interval_s, 90);

        // NVS rung: the override beats the document.
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![(
                Namespace::Schema,
                "sampling.report_interval_s".into(),
                "30".into(),
            )]),
        );
        assert!(warnings.is_empty());
        assert_eq!(config.sampling.report_interval_s, 30);
    }

    #[test]
    fn bad_nvs_value_keeps_the_document_value() {
        let (mut config, _) = resolved(
            r#"
            [sensor]
            id = "0b1ac1e7-38f8-4f3e-9d0a-8b6a2c4e5f70"

            [sampling]
            report_interval_s = 90
            "#,
        );
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![
                (
                    Namespace::Schema,
                    "sampling.report_interval_s".into(),
                    "abc".into(),
                ),
                (Namespace::Identity, "sensor_id".into(), "not-a-uuid".into()),
                (
                    Namespace::Cal,
                    "decibel_calibration_offset_db".into(),
                    "999".into(),
                ),
            ]),
        );
        assert_eq!(warnings.len(), 3, "every skip is explained");
        // The tested document values stand.
        assert_eq!(config.sampling.report_interval_s, 90);
        assert_eq!(config.sensor.id, "0b1ac1e7-38f8-4f3e-9d0a-8b6a2c4e5f70");
        assert_eq!(config.decibel.calibration_offset_db, 0.0);
    }

    #[test]
    fn unknown_nvs_keys_are_reported_and_ignored() {
        let (mut config, _) = resolved("");
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![
                (
                    Namespace::Schema,
                    "sampling.report_intervall".into(),
                    "30".into(),
                ),
                (Namespace::Identity, "serial".into(), "x".into()),
                (Namespace::Cal, "temp_offset".into(), "1.0".into()),
            ]),
        );
        assert_eq!(warnings.len(), 3);
        assert!(warnings.iter().all(|w| w.contains("unknown")));
        assert_eq!(config, Config::builtin());
    }

    #[test]
    fn cal_override_applies_in_window() {
        let (mut config, _) = resolved("");
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![(
                Namespace::Cal,
                "decibel_calibration_offset_db".into(),
                "-2.5".into(),
            )]),
        );
        assert!(warnings.is_empty());
        assert_eq!(config.decibel.calibration_offset_db, -2.5);
    }

    #[test]
    fn schema_override_ranges_mirror_the_document() {
        let (mut config, _) = resolved("");
        // Same bounds config.rs enforces: mqtt port 1..=65535 …
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![(
                Namespace::Schema,
                "transport.mqtt.port".into(),
                "70000".into(),
            )]),
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(config.transport.mqtt.port, 8883);

        // … and gpio 0..=48.
        let (mut config, _) = resolved("");
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![(
                Namespace::Schema,
                "sensors.pir.gpio_pin".into(),
                "49".into(),
            )]),
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(config.sensors.pir.gpio_pin, 4);

        // A value inside both windows applies cleanly.
        let (mut config, _) = resolved("");
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![
                (
                    Namespace::Schema,
                    "transport.mqtt.port".into(),
                    "1883".into(),
                ),
                (
                    Namespace::Schema,
                    "sensors.pir.gpio_pin".into(),
                    "17".into(),
                ),
            ]),
        );
        assert!(warnings.is_empty());
        assert_eq!(config.transport.mqtt.port, 1883);
        assert_eq!(config.sensors.pir.gpio_pin, 17);
    }

    #[test]
    fn wire_secrets_never_enter_the_config() {
        let (mut config, _) = resolved("");
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![
                (Namespace::Wire, "wifi_ssid".into(), "venue-guest".into()),
                (Namespace::Wire, "wifi_password".into(), "hunter2".into()),
                (
                    Namespace::Wire,
                    "api_key".into(),
                    "sk-0123456789abcdef".into(),
                ),
            ]),
        );
        // The overrides are acknowledged, not silently dropped…
        assert_eq!(warnings.len(), 3);
        // …but nothing secret-bearing reached the config…
        assert_eq!(config, Config::builtin());
        // …and its serialization stays clean under the tripwire.
        let text = config.to_toml();
        assert!(!config::serialized_output_contains_secret_marker(&text));
    }

    #[test]
    fn no_overrides_change_nothing() {
        let (mut config, _) = resolved("");
        let warnings = overridden(&mut config, VecOverrides::default());
        assert!(warnings.is_empty());
        assert_eq!(config, Config::builtin());

        let (mut config, _) = resolved("");
        let warnings = overridden(&mut config, NoOverrides);
        assert!(warnings.is_empty());
        assert_eq!(config, Config::builtin());
    }

    #[test]
    fn transport_selection_survives_override_round_trip() {
        let (mut config, _) = resolved("");
        let warnings = overridden(
            &mut config,
            VecOverrides(vec![(
                Namespace::Schema,
                "transport.mqtt.broker".into(),
                "mqtt.venue-a.clubins.de".into(),
            )]),
        );
        assert!(warnings.is_empty());
        assert_eq!(config.transport.mqtt.broker, "mqtt.venue-a.clubins.de");
        assert_eq!(config.transport.active, TransportKind::Http);
    }
}
