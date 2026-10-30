//! Host tests for the measurement model — Unity.
//!
//! The tables are contract bytes: the type names must equal the one-topic-
//! per-type wire names, and the result vocabulary must equal status.go's
//! knownResults verbatim (closed set — a drift here publishes a status the
//! server 400s). The model tables and cap_registry's ingest-type mapping
//! are checked against each other so the two sources of truth cannot drift
//! apart silently.

#include "cap_registry.h"
#include "measurement.h"

#include <string.h>

#include "unity.h"

void setUp(void)
{
}

void tearDown(void)
{
}

void test_type_names_are_the_contract_wire_names(void)
{
    static const char *const expected[CLUBINS_MEAS_TYPE_COUNT] = {
        "decibel", "pax", "weather", "co2", "pm25", "light", "status",
    };
    for (size_t i = 0; i < CLUBINS_MEAS_TYPE_COUNT; i++) {
        TEST_ASSERT_EQUAL_STRING(expected[i], clubins_measurement_type_names[i]);
        TEST_ASSERT_EQUAL_STRING(expected[i],
                                 clubins_measurement_type_name((clubins_meas_type_t)i));
    }
    TEST_ASSERT_NULL(clubins_measurement_type_name((clubins_meas_type_t)CLUBINS_MEAS_TYPE_COUNT));
    TEST_ASSERT_NULL(clubins_measurement_type_name((clubins_meas_type_t)-1));
}

void test_ingest_type_follows_the_populated_type(void)
{
    const clubins_measurement_t decibel = {.type = CLUBINS_MEAS_DECIBEL,
                                           .as.decibel = {.dba = 654, .dbc = 700}};
    const clubins_measurement_t pax = {.type = CLUBINS_MEAS_PAX, .as.pax = {.occupancy_count = 3}};
    const clubins_measurement_t status = {.type = CLUBINS_MEAS_STATUS,
                                          .as.status = {.result = CLUBINS_STATUS_SUCCESS}};

    TEST_ASSERT_EQUAL_STRING("decibel", clubins_measurement_ingest_type(&decibel));
    TEST_ASSERT_EQUAL_STRING("pax", clubins_measurement_ingest_type(&pax));
    TEST_ASSERT_EQUAL_STRING("status", clubins_measurement_ingest_type(&status));
    TEST_ASSERT_NULL(clubins_measurement_ingest_type(NULL));
}

void test_result_vocabulary_matches_status_go_verbatim(void)
{
    static const char *const expected[CLUBINS_STATUS_RESULT_COUNT] = {
        "started", "stopped", "success", "degraded", "failed",
    };
    for (size_t i = 0; i < CLUBINS_STATUS_RESULT_COUNT; i++) {
        TEST_ASSERT_EQUAL_STRING(expected[i], clubins_status_result_names[i]);
        TEST_ASSERT_EQUAL_STRING(expected[i],
                                 clubins_status_result_name((clubins_status_result_t)i));
    }
    TEST_ASSERT_NULL(
        clubins_status_result_name((clubins_status_result_t)CLUBINS_STATUS_RESULT_COUNT));
}

void test_result_parse_is_exact_and_case_sensitive(void)
{
    clubins_status_result_t parsed = CLUBINS_STATUS_RESULT_COUNT;
    for (size_t i = 0; i < CLUBINS_STATUS_RESULT_COUNT; i++) {
        TEST_ASSERT_TRUE(clubins_status_result_parse(clubins_status_result_names[i], &parsed));
        TEST_ASSERT_EQUAL_UINT(i, (size_t)parsed);
    }
    /* The server's membership test is exact — near misses must fail. */
    TEST_ASSERT_FALSE(clubins_status_result_parse("Success", &parsed));
    TEST_ASSERT_FALSE(clubins_status_result_parse("ok", &parsed));
    TEST_ASSERT_FALSE(clubins_status_result_parse("", &parsed));
    TEST_ASSERT_FALSE(clubins_status_result_parse("success ", &parsed));
    TEST_ASSERT_FALSE(clubins_status_result_parse(NULL, &parsed));
}

void test_registry_ingest_types_are_measurement_types(void)
{
    /* cap_registry.c maps sensor capabilities to ingest types by its own
     * string literals; both tables must draw from the same contract. */
    const clubins_cap_id_t sensor_caps[] = {
        CLUBINS_CAP_SENSOR_DECIBEL, CLUBINS_CAP_SENSOR_PAX,       CLUBINS_CAP_SENSOR_BME280,
        CLUBINS_CAP_SENSOR_SHT4X,   CLUBINS_CAP_SENSOR_SCD4X,     CLUBINS_CAP_SENSOR_PMS5003,
        CLUBINS_CAP_SENSOR_BH1750,
    };
    for (size_t i = 0; i < sizeof(sensor_caps) / sizeof(sensor_caps[0]); i++) {
        const char *ingest_type = clubins_sensor_ingest_type(sensor_caps[i]);
        TEST_ASSERT_NOT_NULL(ingest_type);
        bool found = false;
        for (size_t t = 0; t < CLUBINS_MEAS_TYPE_COUNT; t++) {
            if (strcmp(ingest_type, clubins_measurement_type_names[t]) == 0) {
                found = true;
                break;
            }
        }
        TEST_ASSERT_TRUE(found);
    }
}

void test_measurement_equal_compares_payload_and_status_strings(void)
{
    const clubins_measurement_t a = {.type = CLUBINS_MEAS_DECIBEL,
                                     .as.decibel = {.dba = 654, .dbc = 700}};
    clubins_measurement_t b = a;
    TEST_ASSERT_TRUE(clubins_measurement_equal(&a, &b));

    b.as.decibel.dba = 655;
    TEST_ASSERT_FALSE(clubins_measurement_equal(&a, &b));

    const clubins_measurement_t c = {.type = CLUBINS_MEAS_PAX, .as.pax = {.occupancy_count = 0}};
    TEST_ASSERT_FALSE(clubins_measurement_equal(&a, &c)); /* different type */

    const clubins_measurement_t s1 = {.type = CLUBINS_MEAS_STATUS,
                                      .as.status = {.result = CLUBINS_STATUS_SUCCESS,
                                                    .subsystem = "power",
                                                    .message = NULL}};
    const clubins_measurement_t s2 = {.type = CLUBINS_MEAS_STATUS,
                                      .as.status = {.result = CLUBINS_STATUS_SUCCESS,
                                                    .subsystem = "power",
                                                    .message = ""}};
    /* NULL and "" render identically on the wire — equal. */
    TEST_ASSERT_TRUE(clubins_measurement_equal(&s1, &s2));

    const clubins_measurement_t s3 = {.type = CLUBINS_MEAS_STATUS,
                                      .as.status = {.result = CLUBINS_STATUS_SUCCESS,
                                                    .subsystem = "power",
                                                    .message = "boot ok"}};
    TEST_ASSERT_FALSE(clubins_measurement_equal(&s1, &s3));
}

void test_weather_has_flags_are_independent(void)
{
    clubins_measurement_t w = {.type = CLUBINS_MEAS_WEATHER, .as.weather = {false}};
    TEST_ASSERT_FALSE(w.as.weather.has_temperature);
    TEST_ASSERT_FALSE(w.as.weather.has_humidity);
    TEST_ASSERT_FALSE(w.as.weather.has_pressure);

    /* SHT4x shape: temperature + humidity, no pressure. */
    w.as.weather.has_temperature = true;
    w.as.weather.temperature_c = 215;
    w.as.weather.has_humidity = true;
    w.as.weather.humidity_pct = 48;
    TEST_ASSERT_TRUE(clubins_measurement_equal(&w, &w));
}

int main(void)
{
    UNITY_BEGIN();
    RUN_TEST(test_type_names_are_the_contract_wire_names);
    RUN_TEST(test_ingest_type_follows_the_populated_type);
    RUN_TEST(test_result_vocabulary_matches_status_go_verbatim);
    RUN_TEST(test_result_parse_is_exact_and_case_sensitive);
    RUN_TEST(test_registry_ingest_types_are_measurement_types);
    RUN_TEST(test_measurement_equal_compares_payload_and_status_strings);
    RUN_TEST(test_weather_has_flags_are_independent);
    return UNITY_END();
}
