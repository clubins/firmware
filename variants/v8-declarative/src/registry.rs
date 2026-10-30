//! The statically-typed registry of pipeline stage kinds.
//!
//! Every kind that may appear in `config/pipelines.toml` has one variant
//! here, and the variant exists only when its capability flag is compiled
//! in. `lookup_source`/`lookup_stage`/`lookup_sink` map the declaration's
//! kind *names* to typed arms; the graph walks declarations through these
//! lookups, so a pipeline that names a kind the binary does not contain
//! fails boot validation with an `Unknown*Kind` error. That is the V8
//! division of labour: **compile-time flags decide which kinds exist;
//! runtime TOML decides which instances run.**
//!
//! Kinds only *validate* their parameters into typed config here — runtime
//! behaviour arrives with the driver PRs (`docs/plan/build-dag.md`). The
//! `Pipe` source and the `Aggregate`/`Throttle` generic stages are the
//! graph's plumbing and are always compiled in, like the internal plumbing
//! feature flags they correspond to.

use alloc::string::String;
use alloc::vec::Vec;

use crate::decl::{ParamError, ParamGet, Params};

/// Kinds of pipeline sources. `Pipe` is unconditional plumbing — it sources
/// one pipeline from another's stream and is what makes the declaration set
/// a graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Pipe,
    /// SPK0641HT4H-1 PDM microphone, I2S0 @ 48 kHz (`sensor-decibel`).
    #[cfg(feature = "sensor-decibel")]
    Decibel,
    /// PIR occupancy front-end on a config-chosen GPIO pin (`sensor-pax`).
    #[cfg(feature = "sensor-pax")]
    Pir,
    /// BME280 over I2C (`sensor-bme280`).
    #[cfg(feature = "sensor-bme280")]
    Bme280,
    /// SHT4x over I2C (`sensor-sht4x`).
    #[cfg(feature = "sensor-sht4x")]
    Sht4x,
    /// SCD4x CO₂ over I2C (`sensor-scd4x`).
    #[cfg(feature = "sensor-scd4x")]
    Scd4x,
    /// PMS5003 particulates over UART (`sensor-pms5003`).
    #[cfg(feature = "sensor-pms5003")]
    Pms5003,
    /// BH1750 ambient light over I2C (`sensor-bh1750`).
    #[cfg(feature = "sensor-bh1750")]
    Bh1750,
    /// Wall-clock ticks for the status heartbeat (`ops-status`).
    #[cfg(feature = "ops-status")]
    Clock,
}

/// Kinds of pipeline processing stages. The decibel and pax arms are
/// capability-gated; `Aggregate` and `Throttle` are generic plumbing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageKind {
    /// Roll samples up into a longer window (`aggregate`).
    Aggregate,
    /// Pass every *n*-th sample (`throttle`).
    Throttle,
    /// A-weighted level (`sensor-decibel`).
    #[cfg(feature = "sensor-decibel")]
    AWeight,
    /// C-weighted level (`sensor-decibel`).
    #[cfg(feature = "sensor-decibel")]
    CWeight,
    /// LAeq/LCeq window statistics with overload detection
    /// (`sensor-decibel`; per `implementation_notes/decibel.md`).
    #[cfg(feature = "sensor-decibel")]
    WindowStats,
    /// Integrate PIR pulses into occupancy counts (`sensor-pax`).
    #[cfg(feature = "sensor-pax")]
    PaxIntegrate,
}

/// Kinds of pipeline sinks — where finished measurements go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinkKind {
    /// HTTP JSON ingest, the working server contract (`transport-http`).
    #[cfg(feature = "transport-http")]
    HttpIngest,
    /// MQTT v2 protobuf, the designed contract (`transport-mqtt`).
    #[cfg(feature = "transport-mqtt")]
    MqttIngest,
    /// Retained status heartbeat (`ops-status`).
    #[cfg(feature = "ops-status")]
    StatusHeartbeat,
    /// Local diagnostic logging (`logging-defmt`).
    #[cfg(feature = "logging-defmt")]
    LogSink,
}

/// The measurement types the firmware may emit. Mirrors the server's ingest
/// types (`decibel`, `pax`, `weather`, `status`) plus the three new types
/// the spec ships behind the one-topic-per-type rule (`co2`, `pm25`,
/// `light`) — those have no server receiver yet, which is a known,
/// deliberate gap (spec §Decisions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementType {
    Decibel,
    Pax,
    Weather,
    Co2,
    Pm25,
    Light,
}

impl MeasurementType {
    pub fn lookup(name: &str) -> Option<Self> {
        match name {
            "decibel" => Some(Self::Decibel),
            "pax" => Some(Self::Pax),
            "weather" => Some(Self::Weather),
            "co2" => Some(Self::Co2),
            "pm25" => Some(Self::Pm25),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Decibel => "decibel",
            Self::Pax => "pax",
            Self::Weather => "weather",
            Self::Co2 => "co2",
            Self::Pm25 => "pm25",
            Self::Light => "light",
        }
    }
}

/// A per-kind parameter validation failure. The graph adds pipeline context
/// when it turns these into `GraphError::MissingParam`/`BadParam`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamIssue {
    Missing { field: &'static str },
    BadType { field: String, want: &'static str },
    BadValue { field: String, why: String },
    Unknown { field: String },
}

/// One compiled-in stage kind: its canonical name and how to validate a
/// declared instance's parameters.
pub trait Kind: Copy {
    /// The kind name as it appears in `config/pipelines.toml`.
    fn name(&self) -> &'static str;

    /// Validate a declared instance's parameters. Unknown keys are errors —
    /// a typo like `cadance_ms` must not be silently ignored.
    fn validate(&self, params: &Params) -> Result<(), ParamIssue>;
}

/// Reject parameter keys a kind does not understand.
fn no_unknown(params: &Params, known: &[&str]) -> Result<(), ParamIssue> {
    for key in params.keys() {
        if !known.contains(&key.as_str()) {
            return Err(ParamIssue::Unknown { field: key.clone() });
        }
    }
    Ok(())
}

/// Required, strictly positive `u64`.
fn req_positive_u64(params: &Params, field: &'static str) -> Result<u64, ParamIssue> {
    params.u64_param(field).map_err(|e| match e {
        ParamError::Missing => ParamIssue::Missing { field },
        ParamError::BadType { want } => ParamIssue::BadType {
            field: field.into(),
            want,
        },
        ParamError::BadValue => ParamIssue::BadValue {
            field: field.into(),
            why: "must be a positive duration".into(),
        },
    })
}

/// Required `f64` (integers widen; NaN/inf rejected). Only the decibel
/// chain's tunable validation consumes it.
#[cfg(feature = "sensor-decibel")]
fn req_f64(params: &Params, field: &'static str) -> Result<f64, ParamIssue> {
    params.f64_param(field).map_err(|e| match e {
        ParamError::Missing => ParamIssue::Missing { field },
        ParamError::BadType { want } => ParamIssue::BadType {
            field: field.into(),
            want,
        },
        ParamError::BadValue => ParamIssue::BadValue {
            field: field.into(),
            why: "must be a finite number".into(),
        },
    })
}

/// Optional `f64` (NaN/inf rejected when present). Only the decibel
/// sink's tunables consume it.
#[cfg(feature = "sensor-decibel")]
fn opt_f64(params: &Params, field: &'static str) -> Result<(), ParamIssue> {
    if params.get(field).is_some() {
        req_f64(params, field)?;
    }
    Ok(())
}

/// Required measurement type by name. Shared with the runtime's sink
/// builder, which re-reads the validated `type` parameter.
#[cfg(any(feature = "transport-http", feature = "transport-mqtt"))]
pub(crate) fn req_measurement(
    params: &Params,
    field: &'static str,
) -> Result<MeasurementType, ParamIssue> {
    let name = params.str_param(field).map_err(|e| match e {
        ParamError::Missing => ParamIssue::Missing { field },
        ParamError::BadType { want } => ParamIssue::BadType {
            field: field.into(),
            want,
        },
        ParamError::BadValue => unreachable!("str_param never yields BadValue"),
    })?;
    MeasurementType::lookup(&name).ok_or_else(|| ParamIssue::BadValue {
        field: field.into(),
        why: alloc::format!(
            "'{name}' is not a known measurement type (one of decibel, pax, weather, co2, pm25, light)"
        ),
    })
}

impl Kind for SourceKind {
    fn name(&self) -> &'static str {
        match self {
            Self::Pipe => "pipe",
            #[cfg(feature = "sensor-decibel")]
            Self::Decibel => "decibel",
            #[cfg(feature = "sensor-pax")]
            Self::Pir => "pir",
            #[cfg(feature = "sensor-bme280")]
            Self::Bme280 => "bme280",
            #[cfg(feature = "sensor-sht4x")]
            Self::Sht4x => "sht4x",
            #[cfg(feature = "sensor-scd4x")]
            Self::Scd4x => "scd4x",
            #[cfg(feature = "sensor-pms5003")]
            Self::Pms5003 => "pms5003",
            #[cfg(feature = "sensor-bh1750")]
            Self::Bh1750 => "bh1750",
            #[cfg(feature = "ops-status")]
            Self::Clock => "clock",
        }
    }

    fn validate(&self, params: &Params) -> Result<(), ParamIssue> {
        match self {
            // Pipe sources name the pipeline they consume.
            Self::Pipe => {
                no_unknown(params, &["cadence_ms", "pipeline"])?;
                params.str_param("pipeline").map_err(|e| match e {
                    ParamError::Missing => ParamIssue::Missing { field: "pipeline" },
                    ParamError::BadType { want } => ParamIssue::BadType {
                        field: "pipeline".into(),
                        want,
                    },
                    ParamError::BadValue => {
                        unreachable!("str_param never yields BadValue")
                    }
                })?;
                Ok(())
            }
            // Sensor sources take no declared parameters: identity, bus
            // wiring and calibration are per-device NVS/config concerns,
            // not pipeline data (spec §Config).
            #[cfg(feature = "sensor-decibel")]
            Self::Decibel => no_unknown(params, &["cadence_ms"]),
            #[cfg(feature = "sensor-pax")]
            Self::Pir => no_unknown(params, &["cadence_ms"]),
            #[cfg(feature = "sensor-bme280")]
            Self::Bme280 => no_unknown(params, &["cadence_ms"]),
            #[cfg(feature = "sensor-sht4x")]
            Self::Sht4x => no_unknown(params, &["cadence_ms"]),
            #[cfg(feature = "sensor-scd4x")]
            Self::Scd4x => no_unknown(params, &["cadence_ms"]),
            #[cfg(feature = "sensor-pms5003")]
            Self::Pms5003 => no_unknown(params, &["cadence_ms"]),
            #[cfg(feature = "sensor-bh1750")]
            Self::Bh1750 => no_unknown(params, &["cadence_ms"]),
            #[cfg(feature = "ops-status")]
            Self::Clock => no_unknown(params, &["cadence_ms"]),
        }
    }
}

impl Kind for StageKind {
    fn name(&self) -> &'static str {
        match self {
            Self::Aggregate => "aggregate",
            Self::Throttle => "throttle",
            #[cfg(feature = "sensor-decibel")]
            Self::AWeight => "a-weight",
            #[cfg(feature = "sensor-decibel")]
            Self::CWeight => "c-weight",
            #[cfg(feature = "sensor-decibel")]
            Self::WindowStats => "window-stats",
            #[cfg(feature = "sensor-pax")]
            Self::PaxIntegrate => "pax-integrate",
        }
    }

    fn validate(&self, params: &Params) -> Result<(), ParamIssue> {
        match self {
            Self::Aggregate => {
                no_unknown(params, &["window_s"])?;
                let window = req_positive_u64(params, "window_s")?;
                if window == 0 {
                    return Err(ParamIssue::BadValue {
                        field: "window_s".into(),
                        why: "must be a positive duration".into(),
                    });
                }
                Ok(())
            }
            Self::Throttle => {
                no_unknown(params, &["every"])?;
                let every = req_positive_u64(params, "every")?;
                if every == 0 {
                    return Err(ParamIssue::BadValue {
                        field: "every".into(),
                        why: "must be at least 1".into(),
                    });
                }
                Ok(())
            }
            #[cfg(feature = "sensor-decibel")]
            Self::AWeight | Self::CWeight => no_unknown(params, &[]),
            // The decibel chain's tunables, per implementation_notes/decibel.md:
            // the Fast window, the overload ceiling, and the per-unit
            // calibration offset (default 0; real units get theirs from NVS).
            #[cfg(feature = "sensor-decibel")]
            Self::WindowStats => {
                no_unknown(params, &["window_ms", "overload_threshold_db", "offset_db"])?;
                let window = req_positive_u64(params, "window_ms")?;
                if window == 0 {
                    return Err(ParamIssue::BadValue {
                        field: "window_ms".into(),
                        why: "must be a positive duration".into(),
                    });
                }
                // Optional tunables with their decibel.md defaults.
                opt_f64(params, "overload_threshold_db")?;
                opt_f64(params, "offset_db")?;
                Ok(())
            }
            #[cfg(feature = "sensor-pax")]
            Self::PaxIntegrate => no_unknown(params, &[]),
        }
    }
}

impl Kind for SinkKind {
    fn name(&self) -> &'static str {
        // *self: with every sink flag off the enum is empty, and matching a
        // reference to an empty type cannot be exhaustive (a &T is inhabited).
        match *self {
            #[cfg(feature = "transport-http")]
            Self::HttpIngest => "http-ingest",
            #[cfg(feature = "transport-mqtt")]
            Self::MqttIngest => "mqtt-ingest",
            #[cfg(feature = "ops-status")]
            Self::StatusHeartbeat => "status-heartbeat",
            #[cfg(feature = "logging-defmt")]
            Self::LogSink => "log",
        }
    }

    fn validate(&self, params: &Params) -> Result<(), ParamIssue> {
        match *self {
            #[cfg(feature = "transport-http")]
            Self::HttpIngest => {
                no_unknown(params, &["type"])?;
                req_measurement(params, "type")?;
                Ok(())
            }
            #[cfg(feature = "transport-mqtt")]
            Self::MqttIngest => {
                no_unknown(params, &["type", "tls"])?;
                req_measurement(params, "type")?;
                // TLS is the default (broker :8883); it requires the mTLS
                // flag because client identity comes from NVS certificates
                // (api/mqtt/topics.md: CN = sensor UUID).
                let tls = params.bool_param_or("tls", true).map_err(|e| match e {
                    ParamError::BadType { want } => ParamIssue::BadType {
                        field: "tls".into(),
                        want,
                    },
                    _ => unreachable!("bool_param_or only yields Missing/BadType"),
                })?;
                if tls && !cfg!(feature = "transport-mtls") {
                    return Err(ParamIssue::BadValue {
                        field: "tls".into(),
                        why: "true requires the transport-mtls flag (client certificates); \
                              plain-broker setups must set tls = false"
                            .into(),
                    });
                }
                Ok(())
            }
            #[cfg(feature = "ops-status")]
            Self::StatusHeartbeat => no_unknown(params, &[]),
            #[cfg(feature = "logging-defmt")]
            Self::LogSink => {
                no_unknown(params, &["level"])?;
                if let Some(level) = params.get("level") {
                    match level {
                        crate::decl::Scalar::Str(s)
                            if matches!(
                                s.as_str(),
                                "debug" | "info" | "warn" | "error"
                            ) => {}
                        _ => {
                            return Err(ParamIssue::BadValue {
                                field: "level".into(),
                                why: "must be one of debug, info, warn, error".into(),
                            });
                        }
                    }
                }
                Ok(())
            }
            // All-off: no sink kind exists, so no parameters can reach a
            // schema. The arm keeps the match exhaustive and the function
            // signature's parameter used.
            #[cfg(not(any(
                feature = "transport-http",
                feature = "transport-mqtt",
                feature = "ops-status",
                feature = "logging-defmt"
            )))]
            _ => {
                let _ = params;
                Ok(())
            }
        }
    }
}

/// Registry lookups: kind name → typed arm, `None` when the binary does not
/// contain that kind (either it never existed, or its flag is off).
pub fn lookup_source(name: &str) -> Option<SourceKind> {
    match name {
        "pipe" => Some(SourceKind::Pipe),
        #[cfg(feature = "sensor-decibel")]
        "decibel" => Some(SourceKind::Decibel),
        #[cfg(feature = "sensor-pax")]
        "pir" => Some(SourceKind::Pir),
        #[cfg(feature = "sensor-bme280")]
        "bme280" => Some(SourceKind::Bme280),
        #[cfg(feature = "sensor-sht4x")]
        "sht4x" => Some(SourceKind::Sht4x),
        #[cfg(feature = "sensor-scd4x")]
        "scd4x" => Some(SourceKind::Scd4x),
        #[cfg(feature = "sensor-pms5003")]
        "pms5003" => Some(SourceKind::Pms5003),
        #[cfg(feature = "sensor-bh1750")]
        "bh1750" => Some(SourceKind::Bh1750),
        #[cfg(feature = "ops-status")]
        "clock" => Some(SourceKind::Clock),
        _ => None,
    }
}

pub fn lookup_stage(name: &str) -> Option<StageKind> {
    match name {
        "aggregate" => Some(StageKind::Aggregate),
        "throttle" => Some(StageKind::Throttle),
        #[cfg(feature = "sensor-decibel")]
        "a-weight" => Some(StageKind::AWeight),
        #[cfg(feature = "sensor-decibel")]
        "c-weight" => Some(StageKind::CWeight),
        #[cfg(feature = "sensor-decibel")]
        "window-stats" => Some(StageKind::WindowStats),
        #[cfg(feature = "sensor-pax")]
        "pax-integrate" => Some(StageKind::PaxIntegrate),
        _ => None,
    }
}

pub fn lookup_sink(name: &str) -> Option<SinkKind> {
    match name {
        #[cfg(feature = "transport-http")]
        "http-ingest" => Some(SinkKind::HttpIngest),
        #[cfg(feature = "transport-mqtt")]
        "mqtt-ingest" => Some(SinkKind::MqttIngest),
        #[cfg(feature = "ops-status")]
        "status-heartbeat" => Some(SinkKind::StatusHeartbeat),
        #[cfg(feature = "logging-defmt")]
        "log" => Some(SinkKind::LogSink),
        _ => None,
    }
}

/// The kinds this binary contains, for diagnostics and tests. Order is
/// declaration order, not sorted.
// A `vec![]` literal cannot carry per-element `#[cfg]`, so the flag-gated
// arms must push onto a fresh vector.
#[allow(clippy::vec_init_then_push)]
pub fn compiled_sources() -> Vec<&'static str> {
    let mut names = Vec::new();
    // Each arm pushes only when its flag is compiled in.
    names.push("pipe");
    #[cfg(feature = "sensor-decibel")]
    names.push(SourceKind::Decibel.name());
    #[cfg(feature = "sensor-pax")]
    names.push(SourceKind::Pir.name());
    #[cfg(feature = "sensor-bme280")]
    names.push(SourceKind::Bme280.name());
    #[cfg(feature = "sensor-sht4x")]
    names.push(SourceKind::Sht4x.name());
    #[cfg(feature = "sensor-scd4x")]
    names.push(SourceKind::Scd4x.name());
    #[cfg(feature = "sensor-pms5003")]
    names.push(SourceKind::Pms5003.name());
    #[cfg(feature = "sensor-bh1750")]
    names.push(SourceKind::Bh1750.name());
    #[cfg(feature = "ops-status")]
    names.push(SourceKind::Clock.name());
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decl::Scalar;

    fn params(entries: &[(&str, Scalar)]) -> Params {
        entries
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect()
    }

    #[test]
    #[cfg(feature = "sensor-decibel")]
    fn decibel_arm_exists_under_its_flag() {
        // Compiled with sensor-decibel (default) → present.
        assert_eq!(lookup_source("decibel"), Some(SourceKind::Decibel));
        assert_eq!(lookup_stage("window-stats"), Some(StageKind::WindowStats));
    }

    #[test]
    fn decibel_arm_tracks_its_flag() {
        // Holds in every feature world: with the flag the kind exists, and
        // without it the kind does not exist at all (graph tests then force
        // UnknownKind for pipelines naming it).
        let expected = cfg!(feature = "sensor-decibel");
        assert_eq!(lookup_source("decibel").is_some(), expected);
        assert_eq!(lookup_stage("a-weight").is_some(), expected);
    }

    #[test]
    fn never_existed_kinds_are_none_in_every_world() {
        assert_eq!(lookup_source("thermal"), None);
        assert_eq!(lookup_stage("vibrato"), None);
        assert_eq!(lookup_sink("carrier-pigeon"), None);
    }

    #[test]
    fn measurement_types_cover_the_contract() {
        for name in ["decibel", "pax", "weather", "co2", "pm25", "light"] {
            assert!(MeasurementType::lookup(name).is_some(), "{name}");
        }
        assert_eq!(MeasurementType::lookup("db"), None);
        assert_eq!(MeasurementType::lookup("status"), None); // status is a sink, not a type
    }

    #[test]
    #[cfg(feature = "sensor-decibel")]
    fn window_stats_validates_its_tunables() {
        // Missing required window.
        assert_eq!(
            StageKind::WindowStats.validate(&Params::new()),
            Err(ParamIssue::Missing { field: "window_ms" })
        );
        // Zero window is not a duration.
        let zero = params(&[("window_ms", Scalar::Int(0))]);
        assert!(matches!(
            StageKind::WindowStats.validate(&zero),
            Err(ParamIssue::BadValue { field, .. }) if field == "window_ms"
        ));
        // The decibel.md defaults apply when tunables are absent.
        let minimal = params(&[("window_ms", Scalar::Int(125))]);
        assert_eq!(StageKind::WindowStats.validate(&minimal), Ok(()));
        // Present tunables must be finite.
        let with_bad = params(&[
            ("window_ms", Scalar::Int(125)),
            ("overload_threshold_db", Scalar::Float(f64::NAN)),
        ]);
        assert!(matches!(
            StageKind::WindowStats.validate(&with_bad),
            Err(ParamIssue::BadValue { field, .. }) if field == "overload_threshold_db"
        ));
        // Unknown parameters are typos, not defaults.
        let typo = params(&[
            ("window_ms", Scalar::Int(125)),
            ("overload_treshold_db", Scalar::Float(120.0)),
        ]);
        assert!(matches!(
            StageKind::WindowStats.validate(&typo),
            Err(ParamIssue::Unknown { field }) if field == "overload_treshold_db"
        ));
    }
    #[test]
    fn pipe_source_requires_upstream_name() {
        assert_eq!(
            SourceKind::Pipe.validate(&Params::new()),
            Err(ParamIssue::Missing { field: "pipeline" })
        );
        let named = params(&[("pipeline", Scalar::Str("raw".into()))]);
        assert_eq!(SourceKind::Pipe.validate(&named), Ok(()));
    }

    #[test]
    fn sinks_require_a_measurement_type() {
        let no_type = Params::new();
        let typed = params(&[("type", Scalar::Str("decibel".into()))]);
        #[cfg(feature = "transport-http")]
        {
            assert_eq!(
                SinkKind::HttpIngest.validate(&no_type),
                Err(ParamIssue::Missing { field: "type" })
            );
            assert_eq!(SinkKind::HttpIngest.validate(&typed), Ok(()));
            let bad_type = params(&[("type", Scalar::Str("decibels".into()))]);
            assert!(matches!(
                SinkKind::HttpIngest.validate(&bad_type),
                Err(ParamIssue::BadValue { field, .. }) if field == "type"
            ));
        }
        let _ = (no_type, typed);
    }

    #[test]
    fn mqtt_sink_gates_track_the_mtls_flag() {
        // tls defaults to true, so a plain mqtt-ingest sink needs mTLS
        // compiled in; without the flag only tls = false validates, and
        // without transport-mqtt the sink kind does not exist at all.
        #[cfg(feature = "transport-mqtt")]
        {
            let plain = params(&[
                ("type", Scalar::Str("weather".into())),
                ("tls", Scalar::Bool(false)),
            ]);
            assert_eq!(SinkKind::MqttIngest.validate(&plain), Ok(()));
        }
        #[cfg(not(feature = "transport-mqtt"))]
        assert_eq!(lookup_sink("mqtt-ingest"), None);
    }

    #[test]
    fn log_sink_level_is_enumerated() {
        #[cfg(feature = "logging-defmt")]
        {
            let info = params(&[("level", Scalar::Str("info".into()))]);
            assert_eq!(SinkKind::LogSink.validate(&info), Ok(()));
            let loud = params(&[("level", Scalar::Str("LOUD".into()))]);
            assert!(matches!(
                SinkKind::LogSink.validate(&loud),
                Err(ParamIssue::BadValue { field, .. }) if field == "level"
            ));
        }
    }
}
