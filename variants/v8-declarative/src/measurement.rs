//! The typed measurement model: what a pipeline produces and a transport
//! sends.
//!
//! One variant per measurement type, named after the server's ingest types
//! (`server-infra/ingest/service/`): `decibel`, `pax`, `weather`, `status`
//! have receivers today; `co2`, `pm25` ([`Measurement::Particulate`]) and
//! `light` are emitted per the repo's one-topic-per-type rule and land
//! nowhere until the backend grows endpoints (spec §Decisions — a known,
//! deliberate gap). [`Measurement::Environmental`] is the BME280/SHT4x
//! reading, which the spec maps onto the `weather` type with the fields the
//! sensor has (precipitation, rain probability and wind stay unset — those
//! are virtual Open-Meteo fields).
//!
//! Every float is `f32` end to end: the ingest service stores Float32
//! columns and rejects anything a `float32` cannot hold, so the model
//! refuses non-finite values at construction rather than letting a DSP bug
//! become a 400. Occupancy is `u32` by construction; the fractional /
//! out-of-range wire rules live in the codec's decoder, which mirrors the
//! Go handlers exactly.
//!
//! Pure data: `no_std`-compatible with `alloc`, host-tested, and shared by
//! every transport (the HTTP JSON codec gates on `transport-http`; the MQTT
//! protobuf codec arrives with the transport PR).

use alloc::string::String;

/// Why a measurement value was rejected at construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidMeasurement {
    /// A float field was NaN or infinite — the ingest service rejects both
    /// ("must be a finite 32-bit float"), and JSON cannot represent them.
    NotFinite { field: &'static str },
    /// The status subsystem was empty — the service 400s both the absent
    /// and the empty case with "subsystem is required".
    SubsystemRequired,
}

impl core::fmt::Display for InvalidMeasurement {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotFinite { field } => {
                write!(f, "'{field}' must be a finite 32-bit float")
            }
            Self::SubsystemRequired => write!(f, "subsystem is required"),
        }
    }
}

/// The closed vocabulary of `m_status.result` (`status.go` `knownResults`).
///
/// The reason a thing failed belongs in the message, not in a new result
/// value; adding a value here is a deploy, which is the intended friction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusResult {
    Started,
    Stopped,
    Success,
    Degraded,
    Failed,
}

impl StatusResult {
    /// Every legal value, in the server's `knownResults` order.
    pub const ALL: [StatusResult; 5] = [
        StatusResult::Started,
        StatusResult::Stopped,
        StatusResult::Success,
        StatusResult::Degraded,
        StatusResult::Failed,
    ];

    /// The wire spelling of the result.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
            Self::Success => "success",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
        }
    }

    /// Parse a result name; `None` for anything outside the vocabulary.
    pub fn lookup(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|r| r.name() == name)
    }
}

/// A device self-report (`POST /sensors/status`). Every sensor may report on
/// itself, whatever it measures.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusReport {
    /// Row identifier. The server generates a UUIDv7 when absent or
    /// malformed, so the firmware may omit it (`None`) but never invents a
    /// fake one.
    pub status_id: Option<String>,
    /// What part of the device is reporting (`power`, `battery`,
    /// `connection`, …). Deliberately open vocabulary — required, unvalidated.
    pub subsystem: String,
    /// One of the closed [`StatusResult`] values.
    pub result: StatusResult,
    /// Human-readable detail; `None` stays absent on the wire (the server
    /// decodes it as an empty string).
    pub message: Option<String>,
}

impl StatusReport {
    /// A report with an (empty-checked) subsystem and a result. Fill
    /// `status_id` / `message` directly when wanted.
    pub fn new(
        subsystem: &str,
        result: StatusResult,
    ) -> Result<Self, InvalidMeasurement> {
        if subsystem.is_empty() {
            return Err(InvalidMeasurement::SubsystemRequired);
        }
        Ok(Self {
            status_id: None,
            subsystem: subsystem.into(),
            result,
            message: None,
        })
    }

    fn check(&self) -> Result<(), InvalidMeasurement> {
        if self.subsystem.is_empty() {
            return Err(InvalidMeasurement::SubsystemRequired);
        }
        Ok(())
    }
}

/// One finished measurement: the payload a sink encodes and a transport
/// publishes. The shared credentials block (`sensor_id`, `apikey`, `time`)
/// is transport identity, not measurement — the HTTP codec attaches it from
/// device configuration, and MQTT carries identity in the topic instead.
#[derive(Clone, Debug, PartialEq)]
pub enum Measurement {
    /// A-weighted and C-weighted sound levels in dB (`POST
    /// /sensors/decibel`). Produced by the decibel DSP chain.
    Decibel { dba: f32, dbc: f32 },
    /// Integrated occupancy count, 0…2³²−1 (`POST /sensors/pax`).
    Pax { occupancy_count: u32 },
    /// Environmental conditions from the venue sensors (BME280: temperature,
    /// humidity, pressure; SHT4x: temperature, humidity). Publishes to the
    /// weather endpoint with the fields the sensor has — precipitation,
    /// rain probability and wind stay unset (spec §Sensors).
    Environmental {
        temperature: f32,
        humidity: f32,
        pressure: f32,
    },
    /// A full weather reading — all six fields the weather endpoint
    /// requires. Today only the virtual Open-Meteo sensors produce one.
    Weather {
        /// Degrees Celsius.
        temperature: f32,
        /// Percent relative humidity.
        humidity: f32,
        /// Millimetres.
        precipitation: f32,
        /// Percent.
        rain_probability: f32,
        /// km/h.
        wind_speed: f32,
        /// Hectopascals.
        pressure: f32,
    },
    /// CO₂ concentration in ppm (SCD4x). New type per the one-topic-per-type
    /// rule — no server receiver yet.
    Co2 { co2_ppm: f32 },
    /// Particulate concentrations in µg/m³ (PMS5003: PM1.0 / PM2.5 / PM10).
    /// New type — no server receiver yet.
    Particulate { pm1_0: f32, pm2_5: f32, pm10: f32 },
    /// Ambient light in lux (BH1750). New type — no server receiver yet.
    Light { lux: f32 },
    /// Device self-report (`POST /sensors/status`).
    Status(StatusReport),
}

/// Reject non-finite floats — NaN/inf cannot travel as JSON and the service
/// rejects them as "must be a finite 32-bit float".
fn finite(field: &'static str, value: f32) -> Result<(), InvalidMeasurement> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(InvalidMeasurement::NotFinite { field })
    }
}

impl Measurement {
    /// Construct a decibel measurement, rejecting non-finite levels.
    pub fn decibel(dba: f32, dbc: f32) -> Result<Self, InvalidMeasurement> {
        finite("dba", dba)?;
        finite("dbc", dbc)?;
        Ok(Self::Decibel { dba, dbc })
    }

    /// Construct an occupancy measurement. `u32` needs no validation.
    pub fn pax(occupancy_count: u32) -> Self {
        Self::Pax { occupancy_count }
    }

    /// Construct an environmental (BME280/SHT4x) reading.
    pub fn environmental(
        temperature: f32,
        humidity: f32,
        pressure: f32,
    ) -> Result<Self, InvalidMeasurement> {
        finite("temperature", temperature)?;
        finite("humidity", humidity)?;
        finite("pressure", pressure)?;
        Ok(Self::Environmental {
            temperature,
            humidity,
            pressure,
        })
    }

    /// Construct a full six-field weather reading.
    pub fn weather(
        temperature: f32,
        humidity: f32,
        precipitation: f32,
        rain_probability: f32,
        wind_speed: f32,
        pressure: f32,
    ) -> Result<Self, InvalidMeasurement> {
        finite("temperature", temperature)?;
        finite("humidity", humidity)?;
        finite("precipitation", precipitation)?;
        finite("rain_probability", rain_probability)?;
        finite("wind_speed", wind_speed)?;
        finite("pressure", pressure)?;
        Ok(Self::Weather {
            temperature,
            humidity,
            precipitation,
            rain_probability,
            wind_speed,
            pressure,
        })
    }

    /// Construct a CO₂ reading in ppm.
    pub fn co2(co2_ppm: f32) -> Result<Self, InvalidMeasurement> {
        finite("co2_ppm", co2_ppm)?;
        Ok(Self::Co2 { co2_ppm })
    }

    /// Construct a particulate reading in µg/m³.
    pub fn particulate(
        pm1_0: f32,
        pm2_5: f32,
        pm10: f32,
    ) -> Result<Self, InvalidMeasurement> {
        finite("pm1_0", pm1_0)?;
        finite("pm2_5", pm2_5)?;
        finite("pm10", pm10)?;
        Ok(Self::Particulate { pm1_0, pm2_5, pm10 })
    }

    /// Construct an ambient-light reading in lux.
    pub fn light(lux: f32) -> Result<Self, InvalidMeasurement> {
        finite("lux", lux)?;
        Ok(Self::Light { lux })
    }

    /// Construct a status report.
    pub fn status(report: StatusReport) -> Self {
        Self::Status(report)
    }

    /// The measurement type's contract name — the registry's
    /// `MeasurementType` vocabulary plus `status` for self-reports.
    /// Environmental readings report `weather`, the type the spec maps them
    /// to.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Decibel { .. } => "decibel",
            Self::Pax { .. } => "pax",
            Self::Environmental { .. } | Self::Weather { .. } => "weather",
            Self::Co2 { .. } => "co2",
            Self::Particulate { .. } => "pm25",
            Self::Light { .. } => "light",
            Self::Status(_) => "status",
        }
    }

    /// Re-check every invariant (finite floats, non-empty subsystem).
    /// Constructors enforce these; the codec calls this before encoding so a
    /// hand-built struct literal cannot smuggle an invalid value onto the
    /// wire.
    pub fn check(&self) -> Result<(), InvalidMeasurement> {
        match self {
            Self::Decibel { dba, dbc } => {
                finite("dba", *dba)?;
                finite("dbc", *dbc)
            }
            Self::Pax { .. } => Ok(()),
            Self::Environmental {
                temperature,
                humidity,
                pressure,
            } => {
                finite("temperature", *temperature)?;
                finite("humidity", *humidity)?;
                finite("pressure", *pressure)
            }
            Self::Weather {
                temperature,
                humidity,
                precipitation,
                rain_probability,
                wind_speed,
                pressure,
            } => {
                finite("temperature", *temperature)?;
                finite("humidity", *humidity)?;
                finite("precipitation", *precipitation)?;
                finite("rain_probability", *rain_probability)?;
                finite("wind_speed", *wind_speed)?;
                finite("pressure", *pressure)
            }
            Self::Co2 { co2_ppm } => finite("co2_ppm", *co2_ppm),
            Self::Particulate { pm1_0, pm2_5, pm10 } => {
                finite("pm1_0", *pm1_0)?;
                finite("pm2_5", *pm2_5)?;
                finite("pm10", *pm10)
            }
            Self::Light { lux } => finite("lux", *lux),
            Self::Status(report) => report.check(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_floats_are_rejected_at_construction() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                Measurement::decibel(bad, 0.0),
                Err(InvalidMeasurement::NotFinite { field: "dba" })
            );
            assert_eq!(
                Measurement::co2(bad),
                Err(InvalidMeasurement::NotFinite { field: "co2_ppm" })
            );
            assert_eq!(
                Measurement::light(bad),
                Err(InvalidMeasurement::NotFinite { field: "lux" })
            );
        }
        // A check() on a hand-built variant catches the same bug.
        let smuggled = Measurement::Decibel {
            dba: f32::NAN,
            dbc: 0.0,
        };
        assert_eq!(
            smuggled.check(),
            Err(InvalidMeasurement::NotFinite { field: "dba" })
        );
    }

    #[test]
    fn status_result_vocabulary_matches_the_server() {
        // Order and spelling per status.go knownResults.
        for (result, name) in [
            (StatusResult::Started, "started"),
            (StatusResult::Stopped, "stopped"),
            (StatusResult::Success, "success"),
            (StatusResult::Degraded, "degraded"),
            (StatusResult::Failed, "failed"),
        ] {
            assert_eq!(result.name(), name);
            assert_eq!(StatusResult::lookup(name), Some(result));
        }
        // Closed set: the failed/FAILED/fail drift the server polices.
        assert_eq!(StatusResult::lookup("FAILED"), None);
        assert_eq!(StatusResult::lookup("fail"), None);
        assert_eq!(StatusResult::lookup(""), None);
    }

    #[test]
    fn empty_subsystem_is_rejected() {
        assert_eq!(
            StatusReport::new("", StatusResult::Started),
            Err(InvalidMeasurement::SubsystemRequired)
        );
        let mut report = StatusReport::new("power", StatusResult::Degraded)
            .expect("non-empty subsystem");
        report.subsystem = String::new(); // hand-built bypass
        assert_eq!(
            Measurement::status(report).check(),
            Err(InvalidMeasurement::SubsystemRequired)
        );
    }

    #[test]
    fn type_names_use_the_contract_vocabulary() {
        assert_eq!(Measurement::pax(0).type_name(), "pax");
        let db = Measurement::decibel(72.4, 78.1).expect("finite");
        assert_eq!(db.type_name(), "decibel");
        // Environmental readings map onto the weather type (spec §Sensors).
        let env = Measurement::environmental(18.4, 71.0, 1018.4).expect("finite");
        assert_eq!(env.type_name(), "weather");
        let pm = Measurement::particulate(1.0, 2.0, 3.0).expect("finite");
        assert_eq!(pm.type_name(), "pm25");
    }
}
