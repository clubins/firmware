//! HTTP JSON codec for the ingest contracts — the C port of the Rust
//! variants' http_json module.
//!
//! Encodes the request bodies the Go ingest service accepts
//! (server-infra/ingest/service: decibel.go, pax.go, weather.go, status.go)
//! and decodes its responses (service.go writeJSON), with field names byte-
//! identical to the Go structs: `sensor_id`, `apikey`, `time`,
//! `measurement_id`, `dba`, `dbc`, `occupancy_count`, `temperature`,
//! `humidity`, `precipitation`, `rain_probability`, `wind_speed`,
//! `pressure`, `status_id`, `subsystem`, `result`, `message`.
//!
//! One JSON object per POST (the ingest contract: arrays are rejected);
//! caller-buffer encoding, no allocation, no floats — numbers render from
//! the model's fixed-point integers (e.g. 654 tenths-dB → "65.4"). The
//! decoder mirrors the server's own semantics: unknown keys are skipped
//! (encoding/json's default), and a known key with the wrong JSON type is
//! a decode failure, like the server's 400 path.
//!
//! `co2`, `pm25` and `light` are deliberately not encodable here: the HTTP
//! ingest has no endpoint for them — they are MQTT topics (one per type,
//! api/mqtt/topics.md), encoded by the MQTT task with nanopb.
//!
//! Platform-agnostic: no IDF headers — device and host harness compile the
//! same bytes.

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "measurement.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Outcome of an encode or decode call. */
typedef enum {
    CLUBINS_HTTP_JSON_OK = 0,
    /** Encode: the destination buffer is too small; *out_len holds the
     * required length, so the caller can retry once with the right size. */
    CLUBINS_HTTP_JSON_ERR_TRUNCATED = -1,
    /** Encode: the request cannot be rendered as the server contract — a
     * required credential string is NULL, or the measurement type has no
     * HTTP ingest endpoint (co2, pm25, light). */
    CLUBINS_HTTP_JSON_ERR_NOT_ENCODABLE = -2,
    /** Decode: the input is not well-formed JSON. */
    CLUBINS_HTTP_JSON_ERR_MALFORMED = -3,
    /** Decode: a contract key appeared with the wrong JSON type (the
     * server's own decode rule — a string "dba" is a 400, not a zero). */
    CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH = -4,
    /** Decode: a decoded string does not fit the view's bounded buffer. */
    CLUBINS_HTTP_JSON_ERR_FIELD_TOO_LONG = -5,
    /** Decode: a number or string the fixed-point model cannot represent
     * exactly — negative where the model is unsigned, more meaningful
     * fractional digits than the field's scale, exponent notation, a value
     * past the model's width, or a result outside the closed vocabulary.
     * The server may well accept it; the device model cannot hold it. */
    CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE = -6,
    /** NULL or incoherent arguments (no buffer, unknown measurement type). */
    CLUBINS_HTTP_JSON_ERR_BAD_ARGUMENT = -7,
} clubins_http_json_result_t;

/** One HTTP ingest request: the flat credentials every payload carries
 * (service.go `credentials`, promoted by embedding) plus the measurement
 * and its id field. Strings must outlive the call (literals or caller-
 * owned buffers); NULL only where documented. */
typedef struct {
    /** UUID string — the server requires it to parse as one. */
    const char *sensor_id;
    const char *apikey;
    /** RFC3339 UTC timestamp string (RFC3339Nano accepted). */
    const char *time;
    /** Optional for measurements: NULL omits the field and the server
     * generates a UUIDv7 (service.go rowID). */
    const char *measurement_id;
    /** The status payload's id field (status.go `status_id`) — the Go
     * payload names it differently from measurement_id, so does this
     * struct. Ignored for non-status measurements. */
    const char *status_id;
    clubins_measurement_t measurement;
} clubins_http_ingest_request_t;

/** Encode a request body into `buf`. On OK, *out_len is the body length in
 * bytes (compact JSON, no trailing newline; the buffer is NUL-terminated
 * only when the NUL fits). On ERR_TRUNCATED, *out_len is the required
 * length. The body is emitted in the Go structs' declaration order:
 * credentials first, then the id, then the type's fields. Weather emits
 * only the model fields that are set — absent readings are absent from
 * the wire, never zero (the ingest service's own rule). */
clubins_http_json_result_t clubins_http_json_encode_request(
    const clubins_http_ingest_request_t *req, char *buf, size_t cap, size_t *out_len);

/** The ingest service's response body (service.go writeJSON):
 * {"status":"accepted"} on success, {"error":"..."} otherwise. The
 * server's json.Encoder appends a newline; the decoder tolerates it. */
typedef enum {
    CLUBINS_HTTP_RESPONSE_STATUS = 0,
    CLUBINS_HTTP_RESPONSE_ERROR,
    CLUBINS_HTTP_RESPONSE_UNKNOWN, /* neither key present (forward compat) */
} clubins_http_response_kind_t;

/** Bounded copy capacity for decoded response text. The service's own
 * messages are short literals; anything longer truncates — and is
 * reported as truncated, not silently cut. */
#define CLUBINS_HTTP_RESPONSE_TEXT_MAX 96

typedef struct {
    clubins_http_response_kind_t kind;
    char text[CLUBINS_HTTP_RESPONSE_TEXT_MAX]; /* NUL-terminated */
} clubins_http_response_t;

/** Decode a service response body. Overlong text is reported as
 * ERR_FIELD_TOO_LONG (partial copies are not left behind). */
clubins_http_json_result_t clubins_http_json_decode_response(const char *json, size_t len,
                                                             clubins_http_response_t *out);

/** Bounded view capacities for the request decoder: ids/timestamps (UUIDs
 * are 36 chars) and the status free-text fields. */
#define CLUBINS_HTTP_VIEW_ID_MAX 48
#define CLUBINS_HTTP_VIEW_TEXT_MAX 160

/** Decoded view of a request body — the round-trip oracle's read side.
 * The decoder zero-fills the whole view, then fills what the body carries;
 * strings are NUL-terminated copies. */
typedef struct {
    char sensor_id[CLUBINS_HTTP_VIEW_ID_MAX];
    char apikey[CLUBINS_HTTP_VIEW_ID_MAX];
    char time[CLUBINS_HTTP_VIEW_ID_MAX];
    bool has_measurement_id;
    char measurement_id[CLUBINS_HTTP_VIEW_ID_MAX];
    bool has_status_id;
    char status_id[CLUBINS_HTTP_VIEW_ID_MAX];
    clubins_measurement_t measurement;
    char subsystem[CLUBINS_HTTP_VIEW_TEXT_MAX];
    char message[CLUBINS_HTTP_VIEW_TEXT_MAX];
} clubins_http_ingest_request_view_t;

/** Decode a request body into a bounded view — the encoder's mirror, used
 * to verify round-trips (encode → decode → same model) and to conform to
 * server-shaped bodies (unknown keys skipped, wrong types rejected). The
 * first type-specific key fixes the measurement type; a second family's
 * key is a TYPE_MISMATCH, mirroring that one POST carries one type. */
clubins_http_json_result_t clubins_http_json_decode_request(const char *json, size_t len,
                                                            clubins_http_ingest_request_view_t *out);

#ifdef __cplusplus
}
#endif
