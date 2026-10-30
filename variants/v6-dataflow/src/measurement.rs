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
    fn measurement_is_small_enough_for_the_static_rings() {
        // The measurement rings are sized for a POD payload; if the enum
        // grows past two words, revisit ring capacities deliberately.
        assert!(core::mem::size_of::<Measurement>() <= 2 * core::mem::size_of::<usize>());
    }
}
