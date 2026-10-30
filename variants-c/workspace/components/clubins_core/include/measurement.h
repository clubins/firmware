//! The contract-typed measurement model — the C port of the measurement
//! payload the sibling Rust variants carry (V1's shape, V5/V6 field names).
//!
//! Field shapes follow the ingest contracts (spec §Sensor Expansion):
//! decibel, pax, weather and status match the Go ingest structs
//! (server-infra/ingest/service) field-for-field; `co2`, `pm25` and `light`
//! are the new types the firmware emits per the one-topic-per-type rule
//! (spec §Decisions — no server receiver yet, deliberate and tracked).
//! Values are fixed-point integers; there is no float anywhere in the
//! model. Rendering to and parsing from wire text is the codec's job
//! (http_json.h for HTTP, the MQTT/nanopb encoder later).
//!
//! Platform-agnostic: no IDF headers — the device build and the host
//! CTest harness compile these exact bytes.

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Every ingest measurement type, in stable index order — one HTTP path
 * suffix and one MQTT topic per name (api/mqtt/topics.md). The order is
 * part of the contract: per-type MQTT sequence state is indexed by it. */
typedef enum {
    CLUBINS_MEAS_DECIBEL = 0,
    CLUBINS_MEAS_PAX,
    CLUBINS_MEAS_WEATHER,
    CLUBINS_MEAS_CO2,
    CLUBINS_MEAS_PM25,
    CLUBINS_MEAS_LIGHT,
    CLUBINS_MEAS_STATUS,
    CLUBINS_MEAS_TYPE_COUNT
} clubins_meas_type_t;

/** Wire names in clubins_meas_type_t order. The HTTP path suffix and the
 * MQTT topic suffix are the same string (contracts). */
extern const char *const clubins_measurement_type_names[CLUBINS_MEAS_TYPE_COUNT];

/** The wire name of one measurement type; NULL when the index is out of
 * range. */
const char *clubins_measurement_type_name(clubins_meas_type_t type);

/** decibel — A/C-weighted levels in 0.1 dB fixed point: 654 = 65.4 dBA.
 * u16 spans 0…6553.5 dB, far past the mic's ~120 dB SPL clip point. */
typedef struct {
    uint16_t dba;
    uint16_t dbc;
} clubins_decibel_meas_t;

/** pax — PIR detections folded over the sampling window, reported honestly
 * as detections, not a head count (spec §Sensor Expansion). u8 is the
 * window's realistic ceiling; the wire field is whole and non-negative. */
typedef struct {
    uint8_t occupancy_count;
} clubins_pax_meas_t;

/** weather — the device-measurable fields only. Unset fields are absent on
 * the wire, never zero: an absent reading must never be recorded as a
 * genuine measurement of zero (the ingest service's own rule). BME280
 * fills all three; SHT4x leaves pressure unset. The virtual-sensor fields
 * (precipitation, rain_probability, wind_speed) are never set by a device
 * — no device model fields exist for them. */
typedef struct {
    bool has_temperature;
    int16_t temperature_c; /* 0.1 °C fixed point: 215 = 21.5 °C; signed, indoors/outdoors */
    bool has_humidity;
    uint8_t humidity_pct; /* percent */
    bool has_pressure;
    uint16_t pressure_hpa; /* hectopascals */
} clubins_weather_meas_t;

/** co2 — CO₂ concentration in ppm (new type). */
typedef struct {
    uint16_t ppm;
} clubins_co2_meas_t;

/** pm25 — PM2.5 particulate concentration in µg/m³ (new type). */
typedef struct {
    uint16_t ug_m3;
} clubins_pm25_meas_t;

/** light — ambient illuminance in lux (new type). */
typedef struct {
    uint16_t lux;
} clubins_light_meas_t;

/** status result — the server's closed vocabulary (status.go knownResults).
 * A closed set is deliberate: adding a value is a deploy. The reason a
 * thing failed belongs in the free-form message, not in a new result. */
typedef enum {
    CLUBINS_STATUS_STARTED = 0,
    CLUBINS_STATUS_STOPPED,
    CLUBINS_STATUS_SUCCESS,
    CLUBINS_STATUS_DEGRADED,
    CLUBINS_STATUS_FAILED,
    CLUBINS_STATUS_RESULT_COUNT
} clubins_status_result_t;

/** Exact wire strings for the result vocabulary, in enum order. */
extern const char *const clubins_status_result_names[CLUBINS_STATUS_RESULT_COUNT];

/** The wire name of one result; NULL when out of range. */
const char *clubins_status_result_name(clubins_status_result_t result);

/** Exact-name lookup into the vocabulary (case-sensitive, like the
 * server's membership test). Returns false and leaves *out unset when the
 * name is not one of the five. */
bool clubins_status_result_parse(const char *name, clubins_status_result_t *out);

/** status — heartbeat/lifecycle self-report. subsystem is open vocabulary
 * (status.go: "sensors name whatever part of themselves they are reporting
 * on; a new one must not require a deploy"), so it stays a string. Both
 * strings must outlive the measurement: point them at literals or buffers
 * owned by the caller, never at a stack scratch. NULL renders as "". */
typedef struct {
    clubins_status_result_t result;
    const char *subsystem;
    const char *message;
} clubins_status_meas_t;

/** One measurement of any ingest type — the payload that flows from a
 * sampler to a transport. Small and trivially copyable apart from the
 * status strings. */
typedef struct {
    clubins_meas_type_t type;
    union {
        clubins_decibel_meas_t decibel;
        clubins_pax_meas_t pax;
        clubins_weather_meas_t weather;
        clubins_co2_meas_t co2;
        clubins_pm25_meas_t pm25;
        clubins_light_meas_t light;
        clubins_status_meas_t status;
    } as;
} clubins_measurement_t;

/** The wire type name of a populated measurement (the port of V1's
 * `Measurement::ingest_type`). */
const char *clubins_measurement_ingest_type(const clubins_measurement_t *measurement);

/** Shallow structural equality: same type and same payload bytes, with
 * status strings compared by content. The round-trip tests' oracle. */
bool clubins_measurement_equal(const clubins_measurement_t *a, const clubins_measurement_t *b);

#ifdef __cplusplus
}
#endif
