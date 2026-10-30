//! Compile-time feature resolution — the bridge between the cargo flag matrix
//! and the task plan.
//!
//! The 15 public capabilities each map to a cargo feature; the plumbing flags
//! (`driver-i2c`, `driver-uart`, `driver-gpio`, `net-wifi`, `proto-v2`) are
//! dependency edges only and appear here as neither capability nor task.
//! `planned_tasks` is the tested mirror of the spawn arms in
//! `src/device/mod.rs`: the feature matrix builds every flag combination, and
//! the host test in this module pins the plan to the compiled set.

use heapless::Vec;

use crate::config::MAX_TASKS;

/// Public capability flags, in flag-matrix order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    SensorDecibel,
    SensorPax,
    SensorBme280,
    SensorSht4x,
    SensorScd4x,
    SensorPms5003,
    SensorBh1750,
    TransportHttp,
    TransportMqtt,
    TransportMtls,
    Ota,
    Provisioning,
    DeepSleep,
    LoggingDefmt,
    OpsStatus,
}

impl Capability {
    /// The cargo feature name, for logs and future status reporting.
    pub const fn flag(self) -> &'static str {
        match self {
            Self::SensorDecibel => "sensor-decibel",
            Self::SensorPax => "sensor-pax",
            Self::SensorBme280 => "sensor-bme280",
            Self::SensorSht4x => "sensor-sht4x",
            Self::SensorScd4x => "sensor-scd4x",
            Self::SensorPms5003 => "sensor-pms5003",
            Self::SensorBh1750 => "sensor-bh1750",
            Self::TransportHttp => "transport-http",
            Self::TransportMqtt => "transport-mqtt",
            Self::TransportMtls => "transport-mtls",
            Self::Ota => "ota",
            Self::Provisioning => "provisioning",
            Self::DeepSleep => "deep-sleep",
            Self::LoggingDefmt => "logging-defmt",
            Self::OpsStatus => "ops-status",
        }
    }
}

/// Every capability compiled into this build, in flag-matrix order.
#[must_use]
pub fn compiled_capabilities() -> Vec<Capability, 15> {
    let mut caps = Vec::new();
    #[cfg(feature = "sensor-decibel")]
    let _ = caps.push(Capability::SensorDecibel);
    #[cfg(feature = "sensor-pax")]
    let _ = caps.push(Capability::SensorPax);
    #[cfg(feature = "sensor-bme280")]
    let _ = caps.push(Capability::SensorBme280);
    #[cfg(feature = "sensor-sht4x")]
    let _ = caps.push(Capability::SensorSht4x);
    #[cfg(feature = "sensor-scd4x")]
    let _ = caps.push(Capability::SensorScd4x);
    #[cfg(feature = "sensor-pms5003")]
    let _ = caps.push(Capability::SensorPms5003);
    #[cfg(feature = "sensor-bh1750")]
    let _ = caps.push(Capability::SensorBh1750);
    #[cfg(feature = "transport-http")]
    let _ = caps.push(Capability::TransportHttp);
    #[cfg(feature = "transport-mqtt")]
    let _ = caps.push(Capability::TransportMqtt);
    #[cfg(feature = "transport-mtls")]
    let _ = caps.push(Capability::TransportMtls);
    #[cfg(feature = "ota")]
    let _ = caps.push(Capability::Ota);
    #[cfg(feature = "provisioning")]
    let _ = caps.push(Capability::Provisioning);
    #[cfg(feature = "deep-sleep")]
    let _ = caps.push(Capability::DeepSleep);
    #[cfg(feature = "logging-defmt")]
    let _ = caps.push(Capability::LoggingDefmt);
    #[cfg(feature = "ops-status")]
    let _ = caps.push(Capability::OpsStatus);
    caps
}

/// What kind of concern a planned task belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRole {
    /// Samples hardware and publishes measurement events.
    Sensor,
    /// Drains the event queues onto the network.
    Transport,
    /// Reports status, updates, provisioning, power.
    Ops,
}

/// One task the device should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskSpec {
    /// Stable task name for logs and status reporting.
    pub name: &'static str,
    /// The capability that compiled this task in.
    pub capability: Capability,
    pub role: TaskRole,
}

/// Tasks the device main should spawn, in spawn order: sensors first
/// (producers), then transports (consumers), then ops. `logging-defmt`
/// selects a logging transport, not a task, so it plans none.
#[must_use]
pub fn planned_tasks() -> Vec<TaskSpec, MAX_TASKS> {
    let mut tasks = Vec::new();
    #[cfg(feature = "sensor-decibel")]
    let _ = tasks.push(TaskSpec {
        name: "decibel-sensor",
        capability: Capability::SensorDecibel,
        role: TaskRole::Sensor,
    });
    #[cfg(feature = "sensor-pax")]
    let _ = tasks.push(TaskSpec {
        name: "pax-sensor",
        capability: Capability::SensorPax,
        role: TaskRole::Sensor,
    });
    #[cfg(feature = "sensor-bme280")]
    let _ = tasks.push(TaskSpec {
        name: "bme280-sensor",
        capability: Capability::SensorBme280,
        role: TaskRole::Sensor,
    });
    #[cfg(feature = "sensor-sht4x")]
    let _ = tasks.push(TaskSpec {
        name: "sht4x-sensor",
        capability: Capability::SensorSht4x,
        role: TaskRole::Sensor,
    });
    #[cfg(feature = "sensor-scd4x")]
    let _ = tasks.push(TaskSpec {
        name: "scd4x-sensor",
        capability: Capability::SensorScd4x,
        role: TaskRole::Sensor,
    });
    #[cfg(feature = "sensor-pms5003")]
    let _ = tasks.push(TaskSpec {
        name: "pms5003-sensor",
        capability: Capability::SensorPms5003,
        role: TaskRole::Sensor,
    });
    #[cfg(feature = "sensor-bh1750")]
    let _ = tasks.push(TaskSpec {
        name: "bh1750-sensor",
        capability: Capability::SensorBh1750,
        role: TaskRole::Sensor,
    });
    #[cfg(feature = "transport-http")]
    let _ = tasks.push(TaskSpec {
        name: "http-transport",
        capability: Capability::TransportHttp,
        role: TaskRole::Transport,
    });
    #[cfg(feature = "transport-mqtt")]
    let _ = tasks.push(TaskSpec {
        name: "mqtt-transport",
        capability: Capability::TransportMqtt,
        role: TaskRole::Transport,
    });
    #[cfg(feature = "ota")]
    let _ = tasks.push(TaskSpec {
        name: "ota",
        capability: Capability::Ota,
        role: TaskRole::Ops,
    });
    #[cfg(feature = "provisioning")]
    let _ = tasks.push(TaskSpec {
        name: "provisioning",
        capability: Capability::Provisioning,
        role: TaskRole::Ops,
    });
    #[cfg(feature = "deep-sleep")]
    let _ = tasks.push(TaskSpec {
        name: "deep-sleep",
        capability: Capability::DeepSleep,
        role: TaskRole::Ops,
    });
    #[cfg(feature = "ops-status")]
    let _ = tasks.push(TaskSpec {
        name: "ops-status",
        capability: Capability::OpsStatus,
        role: TaskRole::Ops,
    });
    tasks
}

/// Which transport runs, given the runtime selection and the compiled set.
///
/// When both transports are compiled, `config/default.toml`
/// `[transport] active` picks one. When the active transport is not
/// compiled — a config that selects a capability the binary lacks — the
/// other compiled transport runs instead and the device logs the fallback;
/// a config edit can never select what was not compiled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportSelection {
    Http,
    Mqtt,
    /// No transport compiled: events queue until the queues fill, then
    /// backpressure stalls the producers (nothing is lost silently).
    None,
}

#[must_use]
pub fn resolve_transport(active: &str, compiled: &[Capability]) -> TransportSelection {
    let http = compiled.contains(&Capability::TransportHttp);
    let mqtt = compiled.contains(&Capability::TransportMqtt);
    match (http, mqtt) {
        (true, false) => TransportSelection::Http,
        (false, true) => TransportSelection::Mqtt,
        (false, false) => TransportSelection::None,
        // Both compiled: the runtime selection decides; unknown values fall
        // back to http, the documented default (`[transport] active`).
        (true, true) => match active {
            "mqtt" => TransportSelection::Mqtt,
            _ => TransportSelection::Http,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(compiled: &[Capability]) -> Vec<Capability, 15> {
        compiled.iter().copied().collect()
    }

    #[test]
    fn flag_names_are_the_cargo_feature_names() {
        let expected: [(Capability, &str); 15] = [
            (Capability::SensorDecibel, "sensor-decibel"),
            (Capability::SensorPax, "sensor-pax"),
            (Capability::SensorBme280, "sensor-bme280"),
            (Capability::SensorSht4x, "sensor-sht4x"),
            (Capability::SensorScd4x, "sensor-scd4x"),
            (Capability::SensorPms5003, "sensor-pms5003"),
            (Capability::SensorBh1750, "sensor-bh1750"),
            (Capability::TransportHttp, "transport-http"),
            (Capability::TransportMqtt, "transport-mqtt"),
            (Capability::TransportMtls, "transport-mtls"),
            (Capability::Ota, "ota"),
            (Capability::Provisioning, "provisioning"),
            (Capability::DeepSleep, "deep-sleep"),
            (Capability::LoggingDefmt, "logging-defmt"),
            (Capability::OpsStatus, "ops-status"),
        ];
        for (capability, flag) in expected {
            assert_eq!(capability.flag(), flag);
        }
    }

    #[test]
    fn every_planned_task_came_from_a_compiled_capability() {
        let compiled = compiled_capabilities();
        let tasks = planned_tasks();
        for task in &tasks {
            assert!(
                compiled.contains(&task.capability),
                "task {} planned for uncompiled capability {}",
                task.name,
                task.capability.flag()
            );
        }
    }

    #[test]
    fn planned_task_counts_match_the_compiled_feature_set() {
        // cfg! is evaluated per feature combination, so the flag matrix's
        // all-off / each-leaf / all-on builds each run this with new counts.
        let sensor_flags = [
            cfg!(feature = "sensor-decibel"),
            cfg!(feature = "sensor-pax"),
            cfg!(feature = "sensor-bme280"),
            cfg!(feature = "sensor-sht4x"),
            cfg!(feature = "sensor-scd4x"),
            cfg!(feature = "sensor-pms5003"),
            cfg!(feature = "sensor-bh1750"),
        ];
        let transport_flags = [
            cfg!(feature = "transport-http"),
            cfg!(feature = "transport-mqtt"),
        ];
        let ops_flags = [
            cfg!(feature = "ota"),
            cfg!(feature = "provisioning"),
            cfg!(feature = "deep-sleep"),
            cfg!(feature = "ops-status"),
        ];

        let tasks = planned_tasks();
        let count = |role: TaskRole| tasks.iter().filter(|t| t.role == role).count();
        assert_eq!(
            count(TaskRole::Sensor),
            sensor_flags.iter().filter(|f| **f).count()
        );
        assert_eq!(
            count(TaskRole::Transport),
            transport_flags.iter().filter(|f| **f).count()
        );
        assert_eq!(
            count(TaskRole::Ops),
            ops_flags.iter().filter(|f| **f).count()
        );
    }

    #[test]
    fn default_build_plans_the_default_feature_set() {
        if !cfg!(feature = "sensor-decibel")
            || !cfg!(feature = "transport-http")
            || !cfg!(feature = "ops-status")
        {
            return; // only meaningful under the default feature set
        }
        let tasks = planned_tasks();
        let names: Vec<&str, MAX_TASKS> = tasks.iter().map(|t| t.name).collect();
        // The plan follows the compiled feature set: the three I²C sensors
        // slot in after the decibel source, ahead of transport and ops.
        let mut expected: Vec<&str, MAX_TASKS> = Vec::new();
        expected.push("decibel-sensor").unwrap();
        if cfg!(feature = "sensor-bme280") {
            expected.push("bme280-sensor").unwrap();
        }
        if cfg!(feature = "sensor-sht4x") {
            expected.push("sht4x-sensor").unwrap();
        }
        if cfg!(feature = "sensor-scd4x") {
            expected.push("scd4x-sensor").unwrap();
        }
        expected.push("http-transport").unwrap();
        expected.push("ops-status").unwrap();
        assert_eq!(names.as_slice(), expected.as_slice());
    }

    #[test]
    fn transport_resolution_follows_the_runtime_selection() {
        let both = caps(&[Capability::TransportHttp, Capability::TransportMqtt]);
        assert_eq!(resolve_transport("http", &both), TransportSelection::Http);
        assert_eq!(resolve_transport("mqtt", &both), TransportSelection::Mqtt);
        // Unknown runtime values fall back to the documented default.
        assert_eq!(resolve_transport("", &both), TransportSelection::Http);
        assert_eq!(
            resolve_transport("carrier-pigeon", &both),
            TransportSelection::Http
        );
    }

    #[test]
    fn transport_resolution_falls_back_when_selection_is_not_compiled() {
        let http_only = caps(&[Capability::TransportHttp]);
        let mqtt_only = caps(&[Capability::TransportMqtt]);
        // active=mqtt but only http compiled: http runs and the device logs.
        assert_eq!(
            resolve_transport("mqtt", &http_only),
            TransportSelection::Http
        );
        assert_eq!(
            resolve_transport("http", &mqtt_only),
            TransportSelection::Mqtt
        );
    }

    #[test]
    fn transport_resolution_reports_none_when_nothing_is_compiled() {
        assert_eq!(
            resolve_transport("http", &caps(&[])),
            TransportSelection::None
        );
    }
}
