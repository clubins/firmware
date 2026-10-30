//! Ingest contract conformance — Unity, driven by the JSON fixtures under
//! host/fixtures/ingest/.
//!
//! Every valid fixture is the byte-exact body a conforming C1 device puts on
//! the wire (compact JSON, the Go structs' declaration order). Each one is
//! decoded and value-checked, re-encoded, and byte-compared against the
//! file — so encode == decode == fixture, exactly. Rejection fixtures pin
//! the server's 400 shapes. Fixture bytes were verified against
//! server-infra/ingest/service (credentials struct, decibel/pax/weather/
//! status payloads, and the json.Encoder response form).

#include "http_json.h"

#include <stdio.h>
#include <string.h>

#include "unity.h"

#ifndef CLUBINS_FIXTURE_DIR
#error "CLUBINS_FIXTURE_DIR must point at host/fixtures/ingest"
#endif

#define FIXTURE_CAP 512

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

/** Load a fixture byte-exact; NUL-terminates for safety, but callers must
 * use the returned length — the bytes past it are not part of the wire. */
static size_t load_fixture(const char *name, char *buf, size_t cap)
{
    char path[256];
    TEST_ASSERT_TRUE(snprintf(path, sizeof(path), "%s/%s", CLUBINS_FIXTURE_DIR, name) > 0);
    FILE *f = fopen(path, "rb");
    TEST_ASSERT_NOT_NULL_MESSAGE(f, path);
    size_t n = fread(buf, 1, cap - 1, f);
    TEST_ASSERT_TRUE_MESSAGE(ferror(f) == 0, "fixture read failed");
    TEST_ASSERT_TRUE_MESSAGE(n < cap - 1 || feof(f), "fixture larger than buffer");
    fclose(f);
    buf[n] = '\0';
    return n;
}

/** Assert credentials and id fields round-trip through the view. */
static void assert_credentials(const clubins_http_ingest_request_view_t *view, bool expect_meas_id)
{
    TEST_ASSERT_EQUAL_STRING(SENSOR_ID, view->sensor_id);
    TEST_ASSERT_EQUAL_STRING(APIKEY, view->apikey);
    TEST_ASSERT_EQUAL_STRING(TIME, view->time);
    TEST_ASSERT_EQUAL_INT(expect_meas_id, view->has_measurement_id);
    if (expect_meas_id) {
        TEST_ASSERT_EQUAL_STRING(MEASUREMENT_ID, view->measurement_id);
    }
}

/** decode(fixture) → values; encode(the same request) → byte-identical. */
static void assert_conforms(const char *name, const clubins_http_ingest_request_t *request)
{
    char fixture[FIXTURE_CAP];
    const size_t fixture_len = load_fixture(name, fixture, sizeof(fixture));

    clubins_http_ingest_request_view_t view;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_request(fixture, fixture_len, &view));
    TEST_ASSERT_TRUE(clubins_measurement_equal(&request->measurement, &view.measurement));
    assert_credentials(&view, request->measurement_id != NULL);
    if (request->measurement.type == CLUBINS_MEAS_STATUS) {
        TEST_ASSERT_FALSE(view.has_measurement_id);
        TEST_ASSERT_TRUE(view.has_status_id);
        TEST_ASSERT_EQUAL_STRING(STATUS_ID, view.status_id);
    }

    char reencoded[FIXTURE_CAP];
    size_t len = 0;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_encode_request(request, reencoded, sizeof(reencoded),
                                                           &len));
    TEST_ASSERT_EQUAL_UINT(fixture_len, len);
    TEST_ASSERT_EQUAL_MEMORY(fixture, reencoded, fixture_len);

    /* And the re-encoded body decodes to the same values again. */
    clubins_http_ingest_request_view_t again;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_request(reencoded, len, &again));
    TEST_ASSERT_TRUE(clubins_measurement_equal(&request->measurement, &again.measurement));
    TEST_ASSERT_EQUAL_STRING(view.sensor_id, again.sensor_id);
}

static void test_fixture_request_decibel(void)
{
    clubins_http_ingest_request_t req = {.sensor_id = SENSOR_ID,
                                         .apikey = APIKEY,
                                         .time = TIME,
                                         .measurement_id = MEASUREMENT_ID,
                                         .status_id = NULL,
                                         .measurement = {.type = CLUBINS_MEAS_DECIBEL,
                                                         .as.decibel = {.dba = 654, .dbc = 700}}};
    assert_conforms("request-decibel.json", &req);
}

static void test_fixture_request_pax(void)
{
    clubins_http_ingest_request_t req = {.sensor_id = SENSOR_ID,
                                         .apikey = APIKEY,
                                         .time = TIME,
                                         .measurement_id = NULL, /* server mints a UUIDv7 */
                                         .status_id = NULL,
                                         .measurement = {.type = CLUBINS_MEAS_PAX,
                                                         .as.pax = {.occupancy_count = 3}}};
    assert_conforms("request-pax.json", &req);

    char fixture[FIXTURE_CAP];
    const size_t len = load_fixture("request-pax.json", fixture, sizeof(fixture));
    clubins_http_ingest_request_view_t view;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_request(fixture, len, &view));
    TEST_ASSERT_FALSE(view.has_measurement_id);
}

static void test_fixture_request_weather_sht4x(void)
{
    clubins_http_ingest_request_t req = {
        .sensor_id = SENSOR_ID,
        .apikey = APIKEY,
        .time = TIME,
        .measurement_id = MEASUREMENT_ID,
        .status_id = NULL,
        .measurement = {.type = CLUBINS_MEAS_WEATHER,
                        .as.weather = {.has_temperature = true,
                                       .temperature_c = 215,
                                       .has_humidity = true,
                                       .humidity_pct = 48,
                                       .has_pressure = false,
                                       .pressure_hpa = 0}}};
    assert_conforms("request-weather-sht4x.json", &req);
}

static void test_fixture_request_weather_bme280(void)
{
    clubins_http_ingest_request_t req = {
        .sensor_id = SENSOR_ID,
        .apikey = APIKEY,
        .time = TIME,
        .measurement_id = MEASUREMENT_ID,
        .status_id = NULL,
        .measurement = {.type = CLUBINS_MEAS_WEATHER,
                        .as.weather = {.has_temperature = true,
                                       .temperature_c = -52,
                                       .has_humidity = true,
                                       .humidity_pct = 0,
                                       .has_pressure = true,
                                       .pressure_hpa = 1013}}};
    assert_conforms("request-weather-bme280.json", &req);
}

static void test_fixture_request_status_success(void)
{
    clubins_http_ingest_request_t req = {.sensor_id = SENSOR_ID,
                                         .apikey = APIKEY,
                                         .time = TIME,
                                         .measurement_id = NULL,
                                         .status_id = STATUS_ID,
                                         .measurement = {.type = CLUBINS_MEAS_STATUS,
                                                         .as.status = {.result =
                                                                           CLUBINS_STATUS_SUCCESS,
                                                                       .subsystem = "power",
                                                                       .message = ""}}};
    assert_conforms("request-status-success.json", &req);
}

static void test_fixture_request_status_failure_escapes(void)
{
    clubins_http_ingest_request_t req = {.sensor_id = SENSOR_ID,
                                         .apikey = APIKEY,
                                         .time = TIME,
                                         .measurement_id = NULL,
                                         .status_id = STATUS_ID,
                                         .measurement = {.type = CLUBINS_MEAS_STATUS,
                                                         .as.status = {.result =
                                                                           CLUBINS_STATUS_FAILED,
                                                                       .subsystem = "power",
                                                                       .message =
                                                                           "rail \"3v3\" dipped\n"
                                                                           "below limit"}}};
    assert_conforms("request-status-failure.json", &req);

    /* The unescaped bytes land intact in the view. */
    char fixture[FIXTURE_CAP];
    const size_t len = load_fixture("request-status-failure.json", fixture, sizeof(fixture));
    clubins_http_ingest_request_view_t view;
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_request(fixture, len, &view));
    TEST_ASSERT_EQUAL_STRING("rail \"3v3\" dipped\nbelow limit", view.message);
}

static void test_fixture_rejections_match_server_400s(void)
{
    static const struct {
        const char *file;
        clubins_http_json_result_t expected;
    } rejects[] = {
        {"reject-wrong-type.json", CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH},
        {"reject-mixed-families.json", CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH},
        {"reject-unknown-result.json", CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE},
        {"reject-exponent.json", CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE},
        {"reject-past-field-width.json", CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE},
        {"reject-trailing-garbage.json", CLUBINS_HTTP_JSON_ERR_MALFORMED},
    };

    for (size_t i = 0; i < sizeof(rejects) / sizeof(rejects[0]); i++) {
        char fixture[FIXTURE_CAP];
        const size_t len = load_fixture(rejects[i].file, fixture, sizeof(fixture));
        clubins_http_ingest_request_view_t view;
        const clubins_http_json_result_t r =
            clubins_http_json_decode_request(fixture, len, &view);
        TEST_ASSERT_EQUAL_INT_MESSAGE(rejects[i].expected, r, rejects[i].file);
    }
}

static void test_fixture_server_responses(void)
{
    char fixture[FIXTURE_CAP];
    size_t len;
    clubins_http_response_t resp;

    /* The server encodes responses with json.Encoder — trailing newline in
     * the fixture bytes. */
    len = load_fixture("response-accepted.json", fixture, sizeof(fixture));
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_response(fixture, len, &resp));
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_RESPONSE_STATUS, resp.kind);
    TEST_ASSERT_EQUAL_STRING("accepted", resp.text);

    len = load_fixture("response-error.json", fixture, sizeof(fixture));
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_JSON_OK,
                          clubins_http_json_decode_response(fixture, len, &resp));
    TEST_ASSERT_EQUAL_INT(CLUBINS_HTTP_RESPONSE_ERROR, resp.kind);
    TEST_ASSERT_EQUAL_STRING("temperature is required", resp.text);
}

int main(void)
{
    UNITY_BEGIN();
    RUN_TEST(test_fixture_request_decibel);
    RUN_TEST(test_fixture_request_pax);
    RUN_TEST(test_fixture_request_weather_sht4x);
    RUN_TEST(test_fixture_request_weather_bme280);
    RUN_TEST(test_fixture_request_status_success);
    RUN_TEST(test_fixture_request_status_failure_escapes);
    RUN_TEST(test_fixture_rejections_match_server_400s);
    RUN_TEST(test_fixture_server_responses);
    return UNITY_END();
}
