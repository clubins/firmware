//! Host tests for the HTTP JSON codec — Unity.
//!
//! Byte-exact expectations for every request family (compact JSON, the Go
//! structs' declaration order), the escaping rules, truncation behavior,
//! decode round-trips, unknown-key tolerance, and the decoder's rejection
//! paths — the same cases the ingest-conformance fixtures pin in commit
//! three, inline here for the codec commit.

#include "http_json.h"

#include <stdio.h>
#include <string.h>

#include "unity.h"

/* Long enough for any body these tests encode. */
#define BUF_CAP 512

void setUp(void)
{
}

void tearDown(void)
{
}

static const char *const SENSOR_ID = "9c3f1e42-6f5a-4b8e-9a1c-2e7f4a3b5c6d";
static const char *const APIKEY = "s3cr3t-k3y";
static const char *const TIME = "2026-10-30T21:15:00.500Z";
static const char *const MEASUREMENT_ID = "0b7e3d52-1c4a-4f8e-8d3a-6b2c9e5f7a10";
static const char *const STATUS_ID = "f1a2b3c4-1111-4222-8333-aabbccddeeff";

static clubins_http_ingest_request_t base_request(clubins_measurement_t measurement)
{
    clubins_http_ingest_request_t req = {.sensor_id = SENSOR_ID,
                                         .apikey = APIKEY,
                                         .time = TIME,
                                         .measurement_id = MEASUREMENT_ID,
                                         .status_id = NULL,
                                         .measurement = measurement};
    return req;
}

/** Encode and fail loudly on anything but a clean, fitting encode. */
static size_t encode_ok(const clubins_http_ingest_request_t *req, char *buf, size_t cap)
{
    size_t len = 0;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_encode_request(req, buf, cap, &len));
    return len;
}

static void test_encode_decibel_exact_bytes(void)
{
    clubins_http_ingest_request_t req = base_request((clubins_measurement_t){
        .type = CLUBINS_MEAS_DECIBEL, .as.decibel = {.dba = 654, .dbc = 700}});
    char buf[BUF_CAP];
    const size_t len = encode_ok(&req, buf, sizeof(buf));

    /* Credentials, then measurement_id, then dba/dbc — decibelPayload's
     * declaration order. Scale-1 rendering is canonical: "70.0", not "70". */
    TEST_ASSERT_EQUAL_STRING(
        "{\"sensor_id\":\"9c3f1e42-6f5a-4b8e-9a1c-2e7f4a3b5c6d\",\"apikey\":\"s3cr3t-k3y\","
        "\"time\":\"2026-10-30T21:15:00.500Z\",\"measurement_id\":"
        "\"0b7e3d52-1c4a-4f8e-8d3a-6b2c9e5f7a10\",\"dba\":65.4,\"dbc\":70.0}",
        buf);
    TEST_ASSERT_EQUAL_UINT(strlen(buf), len);
}

static void test_encode_pax_omits_absent_measurement_id(void)
{
    clubins_http_ingest_request_t req = base_request(
        (clubins_measurement_t){.type = CLUBINS_MEAS_PAX, .as.pax = {.occupancy_count = 3}});
    req.measurement_id = NULL; /* the server then generates a UUIDv7 */
    char buf[BUF_CAP];
    encode_ok(&req, buf, sizeof(buf));

    TEST_ASSERT_EQUAL_STRING(
        "{\"sensor_id\":\"9c3f1e42-6f5a-4b8e-9a1c-2e7f4a3b5c6d\",\"apikey\":\"s3cr3t-k3y\","
        "\"time\":\"2026-10-30T21:15:00.500Z\",\"occupancy_count\":3}",
        buf);
}

static void test_encode_weather_emits_only_set_fields(void)
{
    /* SHT4x shape: temperature + humidity, no pressure — and no virtual
     * fields, which a device never measures. */
    clubins_http_ingest_request_t req =
        base_request((clubins_measurement_t){.type = CLUBINS_MEAS_WEATHER,
                                             .as.weather = {.has_temperature = true,
                                                            .temperature_c = 215,
                                                            .has_humidity = true,
                                                            .humidity_pct = 48}});
    char buf[BUF_CAP];
    encode_ok(&req, buf, sizeof(buf));
    TEST_ASSERT_EQUAL_STRING(
        "{\"sensor_id\":\"9c3f1e42-6f5a-4b8e-9a1c-2e7f4a3b5c6d\",\"apikey\":\"s3cr3t-k3y\","
        "\"time\":\"2026-10-30T21:15:00.500Z\",\"measurement_id\":"
        "\"0b7e3d52-1c4a-4f8e-8d3a-6b2c9e5f7a10\",\"temperature\":21.5,\"humidity\":48}",
        buf);

    /* BME280 shape adds pressure; a negative temperature renders signed. */
    clubins_http_ingest_request_t full =
        base_request((clubins_measurement_t){.type = CLUBINS_MEAS_WEATHER,
                                             .as.weather = {.has_temperature = true,
                                                            .temperature_c = -52,
                                                            .has_humidity = true,
                                                            .humidity_pct = 0,
                                                            .has_pressure = true,
                                                            .pressure_hpa = 1013}});
    encode_ok(&full, buf, sizeof(buf));
    TEST_ASSERT_EQUAL_STRING(
        "{\"sensor_id\":\"9c3f1e42-6f5a-4b8e-9a1c-2e7f4a3b5c6d\",\"apikey\":\"s3cr3t-k3y\","
        "\"time\":\"2026-10-30T21:15:00.500Z\",\"measurement_id\":"
        "\"0b7e3d52-1c4a-4f8e-8d3a-6b2c9e5f7a10\",\"temperature\":-5.2,\"humidity\":0,"
        "\"pressure\":1013}",
        buf);
}

static void test_encode_status_uses_status_id_and_closed_vocabulary(void)
{
    clubins_http_ingest_request_t req = base_request((clubins_measurement_t){
        .type = CLUBINS_MEAS_STATUS,
        .as.status = {.result = CLUBINS_STATUS_SUCCESS, .subsystem = "power", .message = NULL}});
    req.measurement_id = NULL;
    req.status_id = STATUS_ID;
    char buf[BUF_CAP];
    encode_ok(&req, buf, sizeof(buf));

    /* statusPayload's fields: status_id, subsystem, result, message. NULL
     * message renders as "" (the healthy heartbeat is a silent success). */
    TEST_ASSERT_EQUAL_STRING(
        "{\"sensor_id\":\"9c3f1e42-6f5a-4b8e-9a1c-2e7f4a3b5c6d\",\"apikey\":\"s3cr3t-k3y\","
        "\"time\":\"2026-10-30T21:15:00.500Z\",\"status_id\":"
        "\"f1a2b3c4-1111-4222-8333-aabbccddeeff\",\"subsystem\":\"power\",\"result\":\"success\","
        "\"message\":\"\"}",
        buf);
}

static void test_encode_escapes_strings_per_json(void)
{
    clubins_http_ingest_request_t req =
        base_request((clubins_measurement_t){.type = CLUBINS_MEAS_STATUS,
                                             .as.status = {.result = CLUBINS_STATUS_FAILED,
                                                           .subsystem = "upstream",
                                                           .message = "say \"hi\"\n\\"}});
    req.measurement_id = NULL;
    req.status_id = STATUS_ID;
    char buf[BUF_CAP];
    encode_ok(&req, buf, sizeof(buf));

    /* Quote, newline and backslash escape; everything else is literal. */
    TEST_ASSERT_EQUAL_STRING(
        "{\"sensor_id\":\"9c3f1e42-6f5a-4b8e-9a1c-2e7f4a3b5c6d\",\"apikey\":\"s3cr3t-k3y\","
        "\"time\":\"2026-10-30T21:15:00.500Z\",\"status_id\":"
        "\"f1a2b3c4-1111-4222-8333-aabbccddeeff\",\"subsystem\":\"upstream\",\"result\":\"failed\","
        "\"message\":\"say \\\"hi\\\"\\n\\\\\"}",
        buf);
}

static void test_encode_rejects_missing_credentials_and_off_http_types(void)
{
    char buf[BUF_CAP];
    size_t len = 0;

    clubins_http_ingest_request_t no_sensor = base_request(
        (clubins_measurement_t){.type = CLUBINS_MEAS_PAX, .as.pax = {.occupancy_count = 1}});
    no_sensor.sensor_id = NULL; /* JSON null is a server 400 — refuse */
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_NOT_ENCODABLE,
                          clubins_http_json_encode_request(&no_sensor, buf, sizeof(buf), &len));

    clubins_http_ingest_request_t co2 =
        base_request((clubins_measurement_t){.type = CLUBINS_MEAS_CO2, .as.co2 = {.ppm = 812}});
    co2.measurement_id = NULL;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_NOT_ENCODABLE,
                          clubins_http_json_encode_request(&co2, buf, sizeof(buf), &len));
    clubins_http_ingest_request_t pm25 =
        base_request((clubins_measurement_t){.type = CLUBINS_MEAS_PM25, .as.pm25 = {.ug_m3 = 14}});
    pm25.measurement_id = NULL;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_NOT_ENCODABLE,
                          clubins_http_json_encode_request(&pm25, buf, sizeof(buf), &len));
    clubins_http_ingest_request_t light =
        base_request((clubins_measurement_t){.type = CLUBINS_MEAS_LIGHT, .as.light = {.lux = 540}});
    light.measurement_id = NULL;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_NOT_ENCODABLE,
                          clubins_http_json_encode_request(&light, buf, sizeof(buf), &len));
}

static void test_encode_truncation_reports_required_length_and_overruns_nothing(void)
{
    clubins_http_ingest_request_t req = base_request(
        (clubins_measurement_t){.type = CLUBINS_MEAS_PAX, .as.pax = {.occupancy_count = 3}});
    req.measurement_id = NULL;
    char buf[BUF_CAP];
    memset(buf, 0, sizeof(buf));

    /* First a size probe with a zero-capacity buffer. */
    size_t required = 0;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_TRUNCATED,
                          clubins_http_json_encode_request(&req, NULL, 0, &required));

    /* Then a buffer one byte too small: the write must stop at the cap —
     * the canary past it must survive — and the retry must fit. */
    char tight[BUF_CAP];
    const char canary = 'C';
    TEST_ASSERT_TRUE(required >= 1 && required + 1 < sizeof(tight));
    memset(tight, 0, sizeof(tight));
    tight[required] = canary; /* two past the one-byte-short cap; also the
                               * NUL slot of the full retry */
    size_t len = 0;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_TRUNCATED,
                          clubins_http_json_encode_request(&req, tight, required - 1, &len));
    TEST_ASSERT_EQUAL_UINT(required, len);
    TEST_ASSERT_EQUAL_CHAR(canary, tight[required]);

    len = encode_ok(&req, tight, sizeof(tight));
    TEST_ASSERT_EQUAL_UINT(required, len);
    /* The full-buffer retry NUL-terminates right where the canary sat. */
    TEST_ASSERT_EQUAL_CHAR('\0', tight[required]);
}

static void test_decode_round_trips_every_http_type(void)
{
    const clubins_measurement_t cases[] = {
        {.type = CLUBINS_MEAS_DECIBEL, .as.decibel = {.dba = 654, .dbc = 700}},
        {.type = CLUBINS_MEAS_PAX, .as.pax = {.occupancy_count = 3}},
        {.type = CLUBINS_MEAS_WEATHER,
         .as.weather = {.has_temperature = true,
                        .temperature_c = 215,
                        .has_humidity = true,
                        .humidity_pct = 48,
                        .has_pressure = true,
                        .pressure_hpa = 1013}},
        {.type = CLUBINS_MEAS_WEATHER,
         .as.weather = {.has_temperature = true, .temperature_c = -52}},
        {.type = CLUBINS_MEAS_STATUS,
         .as.status = {.result = CLUBINS_STATUS_SUCCESS, .subsystem = "power", .message = ""}},
    };

    for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
        clubins_http_ingest_request_t req = base_request(cases[i]);
        if (req.measurement.type == CLUBINS_MEAS_STATUS) {
            req.measurement_id = NULL;
            req.status_id = STATUS_ID;
        }
        char buf[BUF_CAP];
        encode_ok(&req, buf, sizeof(buf));

        clubins_http_ingest_request_view_t view;
        TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                              clubins_http_json_decode_request(buf, strlen(buf), &view));
        TEST_ASSERT_TRUE(clubins_measurement_equal(&cases[i], &view.measurement));
        TEST_ASSERT_EQUAL_STRING(SENSOR_ID, view.sensor_id);
        TEST_ASSERT_EQUAL_STRING(APIKEY, view.apikey);
        TEST_ASSERT_EQUAL_STRING(TIME, view.time);
        if (req.measurement.type == CLUBINS_MEAS_STATUS) {
            TEST_ASSERT_FALSE(view.has_measurement_id);
            TEST_ASSERT_TRUE(view.has_status_id);
            TEST_ASSERT_EQUAL_STRING(STATUS_ID, view.status_id);
            TEST_ASSERT_EQUAL_STRING("power", view.subsystem);
        } else {
            TEST_ASSERT_TRUE(view.has_measurement_id);
            TEST_ASSERT_EQUAL_STRING(MEASUREMENT_ID, view.measurement_id);
        }
    }
}

static void test_decode_tolerates_unknown_keys_like_the_server(void)
{
    clubins_http_ingest_request_t req = base_request((clubins_measurement_t){
        .type = CLUBINS_MEAS_DECIBEL, .as.decibel = {.dba = 654, .dbc = 700}});
    char buf[BUF_CAP];
    encode_ok(&req, buf, sizeof(buf));

    /* venue_id and location are refused-if-sent on the real path but the
     * server's decoder skips any unknown key — the view mirrors that. */
    char with_unknown[BUF_CAP];
    const int written = snprintf(
        with_unknown, sizeof(with_unknown),
        "{\"venue_id\":\"v\",\"location\":\"l\",\"future\":{\"a\":[1,2,null,true]},%s", buf + 1);
    TEST_ASSERT_TRUE(written > 0 && (size_t)written < sizeof(with_unknown));

    clubins_http_ingest_request_view_t view;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK, clubins_http_json_decode_request(
                                                    with_unknown, strlen(with_unknown), &view));
    TEST_ASSERT_EQUAL_STRING(SENSOR_ID, view.sensor_id);
    TEST_ASSERT_EQUAL_UINT(654, view.measurement.as.decibel.dba);
}

static void test_decode_rejects_wrong_json_types_on_known_keys(void)
{
    clubins_http_ingest_request_view_t view;
    clubins_http_json_result_t r;

    r = clubins_http_json_decode_request("{\"dba\":\"65.4\"}", strlen("{\"dba\":\"65.4\"}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH, r);

    r = clubins_http_json_decode_request("{\"occupancy_count\":true}",
                                         strlen("{\"occupancy_count\":true}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH, r);

    r = clubins_http_json_decode_request("{\"result\":7}", strlen("{\"result\":7}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH, r);

    r = clubins_http_json_decode_request("{\"sensor_id\":42}", strlen("{\"sensor_id\":42}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH, r);

    /* A second family's key in one body: one POST carries one type. */
    const char *mixed = "{\"dba\":1,\"subsystem\":\"power\"}";
    r = clubins_http_json_decode_request(mixed, strlen(mixed), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH, r);
}

static void test_decode_rejects_unrepresentable_values(void)
{
    clubins_http_ingest_request_view_t view;
    clubins_http_json_result_t r;

    /* Negative where the model is unsigned (the server would store it). */
    r = clubins_http_json_decode_request("{\"occupancy_count\":-1}",
                                         strlen("{\"occupancy_count\":-1}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE, r);

    /* A meaningful digit beyond the field's scale. */
    r = clubins_http_json_decode_request("{\"dba\":65.43}", strlen("{\"dba\":65.43}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE, r);

    /* Exponent notation is refused outright. */
    r = clubins_http_json_decode_request("{\"dba\":6.54e1}", strlen("{\"dba\":6.54e1}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE, r);

    /* Past the model field's width (the wire allows 2^32-1; the model's
     * PIR window is u8). */
    r = clubins_http_json_decode_request("{\"occupancy_count\":300}",
                                         strlen("{\"occupancy_count\":300}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE, r);

    /* Outside the closed result vocabulary. */
    r = clubins_http_json_decode_request("{\"result\":\"broke\"}", strlen("{\"result\":\"broke\"}"),
                                         &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE, r);

    /* Past the model field's width for pressure (u16 tenths of nothing —
     * hPa fits easily; 100000 hPa does not). */
    r = clubins_http_json_decode_request("{\"pressure\":100000}", strlen("{\"pressure\":100000}"),
                                         &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE, r);
}

static void test_decode_accepts_representable_extra_zeros(void)
{
    clubins_http_ingest_request_view_t view;
    /* "65.40" is exactly 654 tenths — representable. */
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_request("{\"dba\":65.40}", 13, &view));
    TEST_ASSERT_EQUAL_UINT(654, view.measurement.as.decibel.dba);
    TEST_ASSERT_EQUAL(CLUBINS_MEAS_DECIBEL, view.measurement.type);
}

static void test_decode_response_status_error_and_whitespace(void)
{
    clubins_http_response_t resp;

    /* The server's json.Encoder appends a newline — tolerated. */
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK, clubins_http_json_decode_response(
                                                    "{\"status\":\"accepted\"}\n",
                                                    strlen("{\"status\":\"accepted\"}\n"), &resp));
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_RESPONSE_STATUS, resp.kind);
    TEST_ASSERT_EQUAL_STRING("accepted", resp.text);

    const char *err_body = "{\"error\":\"temperature is required\"}";
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_response(err_body, strlen(err_body), &resp));
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_RESPONSE_ERROR, resp.kind);
    TEST_ASSERT_EQUAL_STRING("temperature is required", resp.text);

    /* Whitespace-formatted bodies decode like the bytes do. */
    const char *ws_body = " { \"status\" : \"ok\" } ";
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_response(ws_body, strlen(ws_body), &resp));
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_RESPONSE_STATUS, resp.kind);
    TEST_ASSERT_EQUAL_STRING("ok", resp.text);

    /* Neither key: unknown, forward-compatible. */
    const char *warn_body = "{\"warning\":\"x\"}";
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_response(warn_body, strlen(warn_body), &resp));
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_RESPONSE_UNKNOWN, resp.kind);
}

static void test_decode_response_field_too_long_is_reported(void)
{
    clubins_http_response_t resp;
    const char *long_error = "{\"error\":\"0123456789012345678901234567890123456789"
                             "01234567890123456789012345678901234567890123456789012345678901234567"
                             "89\"}";
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_FIELD_TOO_LONG,
                          clubins_http_json_decode_response(long_error, strlen(long_error), &resp));
}

static void test_decode_rejects_malformed_json(void)
{
    clubins_http_ingest_request_view_t view;
    clubins_http_response_t resp;
    clubins_http_json_result_t r;

    r = clubins_http_json_decode_request("", 0, &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_MALFORMED, r);
    r = clubins_http_json_decode_request("{", 1, &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_MALFORMED, r);
    r = clubins_http_json_decode_request("{\"sensor_id\"}", strlen("{\"sensor_id\"}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_MALFORMED, r);
    r = clubins_http_json_decode_request("{\"a\":}", strlen("{\"a\":}"), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_MALFORMED, r);
    /* Trailing garbage after a complete body. */
    const char *trailing = "{\"status\":\"ok\"} x";
    r = clubins_http_json_decode_response(trailing, strlen(trailing), &resp);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_MALFORMED, r);
    /* Raw control characters in strings are rejected, like the server. */
    const char *raw_tab = "{\"sensor_id\":\"a\tb\"}";
    r = clubins_http_json_decode_request(raw_tab, strlen(raw_tab), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_MALFORMED, r);
    /* A dangling escape. */
    const char *dangling = "{\"sensor_id\":\"a\\";
    r = clubins_http_json_decode_request(dangling, strlen(dangling), &view);
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_ERR_MALFORMED, r);
}

static void test_decode_unescapes_strings(void)
{
    clubins_http_ingest_request_view_t view;
    const char *body = "{\"subsystem\":\"power\",\"result\":\"failed\",\"message\":\"line\\nbreak "
                       "\\u00e9 \\\"q\\\"\"}";
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_request(body, strlen(body), &view));
    TEST_ASSERT_EQUAL_STRING("line\nbreak \xC3\xA9 \"q\"", view.message);
    TEST_ASSERT_EQUAL(CLUBINS_MEAS_STATUS, view.measurement.type);
    TEST_ASSERT_EQUAL_INT(CLUBINS_STATUS_FAILED, view.measurement.as.status.result);
}

int main(void)
{
    UNITY_BEGIN();
    RUN_TEST(test_encode_decibel_exact_bytes);
    RUN_TEST(test_encode_pax_omits_absent_measurement_id);
    RUN_TEST(test_encode_weather_emits_only_set_fields);
    RUN_TEST(test_encode_status_uses_status_id_and_closed_vocabulary);
    RUN_TEST(test_encode_escapes_strings_per_json);
    RUN_TEST(test_encode_rejects_missing_credentials_and_off_http_types);
    RUN_TEST(test_encode_truncation_reports_required_length_and_overruns_nothing);
    RUN_TEST(test_decode_round_trips_every_http_type);
    RUN_TEST(test_decode_tolerates_unknown_keys_like_the_server);
    RUN_TEST(test_decode_rejects_wrong_json_types_on_known_keys);
    RUN_TEST(test_decode_rejects_unrepresentable_values);
    RUN_TEST(test_decode_accepts_representable_extra_zeros);
    RUN_TEST(test_decode_response_status_error_and_whitespace);
    RUN_TEST(test_decode_response_field_too_long_is_reported);
    RUN_TEST(test_decode_rejects_malformed_json);
    RUN_TEST(test_decode_unescapes_strings);
    return UNITY_END();
}
