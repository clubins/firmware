//! The contract-typed measurement payload that flows through the graph.
//!
//! Field shapes follow the ingest contracts (spec §Sensor Expansion):
//! decibel, pax, weather and status match the Go ingest structs
//! field-for-field (values fixed-point; string rendering is the transport
//! PR's fixed-buffer job); `co2`, `pm25` and `light` are the new types the
//! firmware emits per the repo's one-topic-per-type rule (spec §Decisions —
//! no server receiver yet, deliberate and tracked).

/// One measurement of any ingest type. `Copy` and small — it rides the
/// static rings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measurement {
    /// `decibel`: A/C-weighted levels, 0.1 dB fixed point (654 = 65.4 dBA).
    /// Server fields: `dba`, `dbc`.
    Decibel { dba: u16, dbc: u16 },
    /// `pax`: PIR detections folded over a window. Server field:
    /// `occupancy_count`. Reports detections honestly — not a head count.
    Pax { occupancy_count: u8 },
    /// `weather`: temperature 0.1 °C fixed point, humidity %, pressure hPa.
    /// BME280 fills all three; SHT4x leaves pressure unset; the Open-Meteo
    /// virtual fields (precipitation, rain probability, wind speed) are
    /// never set by a device.
    Weather {
        temperature_c: Option<i16>,
        humidity_pct: Option<u8>,
        pressure_hpa: Option<u16>,
    },
    /// `co2`: CO₂ in ppm (new type).
    Co2 { ppm: u16 },
    /// `pm25`: PM2.5 in µg/m³ (new type).
    Pm25 { ug_m3: u16 },
    /// `light`: ambient light in lux (new type).
    Light { lux: u16 },
    /// `status`: heartbeat/lifecycle. Server fields: `subsystem`, `result`,
    /// `message` (the free-form message is rendered by the transport PR).
    Status { subsystem: u8, result: u8 },
}

impl Measurement {
    /// The wire type name: HTTP path suffix (`/v1/sensors/<type>`) and MQTT
    /// topic suffix (`v2/sensors/<uuid>/data/<type>`).
    pub const fn ingest_type(&self) -> &'static str {
        match self {
            Self::Decibel { .. } => "decibel",
            Self::Pax { .. } => "pax",
            Self::Weather { .. } => "weather",
            Self::Co2 { .. } => "co2",
            Self::Pm25 { .. } => "pm25",
            Self::Light { .. } => "light",
            Self::Status { .. } => "status",
        }
    }

    /// Stable per-type index — the MQTT drain's per-topic sequence table
    /// is indexed by this (one topic per type, per `api/mqtt/topics.md`).
    /// Order matches [`MEASUREMENT_TYPE_NAMES`].
    pub const fn type_index(&self) -> usize {
        match self {
            Self::Decibel { .. } => 0,
            Self::Pax { .. } => 1,
            Self::Weather { .. } => 2,
            Self::Co2 { .. } => 3,
            Self::Pm25 { .. } => 4,
            Self::Light { .. } => 5,
            Self::Status { .. } => 6,
        }
    }
}

/// Number of distinct ingest types; sizes the per-type MQTT state.
pub const MEASUREMENT_TYPES: usize = 7;

/// Wire names in [`Measurement::type_index`] order — one MQTT topic per
/// name, and the HTTP path suffix is the same string.
pub const MEASUREMENT_TYPE_NAMES: [&str; MEASUREMENT_TYPES] = [
    "decibel", "pax", "weather", "co2", "pm25", "light", "status",
];

/// The server's closed vocabulary for the status `result` field
/// (`server-infra/ingest/service/status.go` `knownResults`, in file order).
/// Adding a value there is a server deploy — the intended friction — so the
/// firmware pins the same five strings; anything else the server rejects
/// with 400, and the transports must never send it.
pub const STATUS_RESULTS: [&str; STATUS_RESULT_COUNT] =
    ["started", "stopped", "success", "degraded", "failed"];

/// Number of server-known status results; sizes [`STATUS_RESULTS`].
pub const STATUS_RESULT_COUNT: usize = 5;

/// `result` index for a started lifecycle event ([`STATUS_RESULTS`] order).
pub const RESULT_STARTED: u8 = 0;
/// `result` index for a stopped lifecycle event.
pub const RESULT_STOPPED: u8 = 1;
/// `result` index for a healthy or successfully-completed report.
pub const RESULT_SUCCESS: u8 = 2;
/// `result` index for a degraded-but-up report.
pub const RESULT_DEGRADED: u8 = 3;
/// `result` index for a failure report (the reason goes in the message).
pub const RESULT_FAILED: u8 = 4;

/// The wire string for a status `result` index ([`Measurement::Status`]'s
/// `result` field), or `None` when the index is outside the server's
/// closed vocabulary.
pub const fn result_name(result: u8) -> Option<&'static str> {
    if (result as usize) < STATUS_RESULT_COUNT {
        Some(STATUS_RESULTS[result as usize])
    } else {
        None
    }
}

/// The closed-vocabulary index of a status `result` wire string, or `None`
/// when the string is not one of the server's five values (the server
/// answers 400 for those — `result` is a closed set, `subsystem` is open).
pub fn result_index(name: &str) -> Option<u8> {
    STATUS_RESULTS
        .iter()
        .position(|known| *known == name)
        .map(|index| index as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_type_mapping_matches_the_contracts() {
        let cases: [(Measurement, &str); 7] = [
            (Measurement::Decibel { dba: 654, dbc: 700 }, "decibel"),
            (Measurement::Pax { occupancy_count: 3 }, "pax"),
            (
                Measurement::Weather {
                    temperature_c: Some(215),
                    humidity_pct: Some(48),
                    pressure_hpa: Some(1013),
                },
                "weather",
            ),
            (Measurement::Co2 { ppm: 812 }, "co2"),
            (Measurement::Pm25 { ug_m3: 14 }, "pm25"),
            (Measurement::Light { lux: 540 }, "light"),
            (
                Measurement::Status {
                    subsystem: 0,
                    result: 0,
                },
                "status",
            ),
        ];
        for (m, ty) in cases {
            assert_eq!(m.ingest_type(), ty);
        }
    }

    #[test]
    fn type_index_and_name_table_agree() {
        // Every type's index lands on its own name in the table — the MQTT
        // per-topic seq table is indexed by this, so a mismatch would
        // publish a measurement under another type's topic.
        let all = [
            Measurement::Decibel { dba: 0, dbc: 0 },
            Measurement::Pax { occupancy_count: 0 },
            Measurement::Weather {
                temperature_c: None,
                humidity_pct: None,
                pressure_hpa: None,
            },
            Measurement::Co2 { ppm: 0 },
            Measurement::Pm25 { ug_m3: 0 },
            Measurement::Light { lux: 0 },
            Measurement::Status {
                subsystem: 0,
                result: 0,
            },
        ];
        for m in all {
            assert_eq!(MEASUREMENT_TYPE_NAMES[m.type_index()], m.ingest_type());
        }
    }

    #[test]
    fn status_result_vocabulary_matches_the_server() {
        // The closed set and its order come from status.go's knownResults.
        // Both directions are tested: the heartbeat renders `success`, and
        // decode maps every server string back to its index.
        let server_order = ["started", "stopped", "success", "degraded", "failed"];
        for (index, name) in server_order.iter().enumerate() {
            assert_eq!(result_name(index as u8), Some(*name));
            assert_eq!(result_index(name), Some(index as u8));
        }
        // Outside the vocabulary: the strings the server answers 400 for
        // (status_test.go TestStatusResultVocabularyIsEnforced).
        for rejected in ["FAILED", "Failed", "fail", "error", "ok", ""] {
            assert_eq!(result_index(rejected), None);
        }
        // Indices beyond the table have no name.
        assert_eq!(result_name(STATUS_RESULT_COUNT as u8), None);
        assert_eq!(result_name(255), None);
    }

    #[test]
    fn measurement_is_small_enough_for_the_static_rings() {
        // The measurement rings are sized for a POD payload; if the enum
        // grows past two words, revisit ring capacities deliberately.
        assert!(core::mem::size_of::<Measurement>() <= 2 * core::mem::size_of::<usize>());
    }
}
