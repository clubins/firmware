//! Host tests for the runtime configuration — Unity.
//!
//! The four verification groups from the task brief, all through the
//! public config.h surface:
//!
//! 1. Precedence order — override tier > file tier > built-ins, every
//!    pairing, partial tiers included.
//! 2. Unknown-key tolerance — unrecognised keys (and unparseable values)
//!    land in the warn list and never reach the pool; the list truncates
//!    at capacity instead of lying.
//! 3. Secret exclusion — the loader never queries secret-shaped keys, and
//!    a dump of the pool contains no secret field markers even when the
//!    file or a source is hostile with them.
//! 4. Round-trip parse — dump → parse reproduces the pool exactly, for
//!    the committed config/default.toml and for mutated pools.
//!
//! The override source is a mock keyed by dotted paths (the same seam the
//! device binds to NVS in nvs_overrides.c), with per-entry error
//! injection and a query log for the never-queries-secrets assertion.

#include "config.h"

#include <stdio.h>
#include <string.h>

#include "unity.h"

void setUp(void)
{
}

void tearDown(void)
{
}

/* ------------------------------------------------------------------ */
/* Mock override source                                                */
/* ------------------------------------------------------------------ */

typedef struct {
    const char *key;
    char type; /* 's' string, 'i' int32, 'f' double */
    const char *sval;
    int32_t ival;
    double fval;
    int rc; /* what the getter returns when the key matches */
} entry_t;

static entry_t g_entries[16];
static size_t g_entry_count;

/** Every dotted key the loader asked for (bound 32 per load). */
static char g_queries[32][64];
static size_t g_query_count;

static void mock_reset(void)
{
    g_entry_count = 0;
    g_query_count = 0;
}

static void mock_add(entry_t e)
{
    g_entries[g_entry_count++] = e;
}

static const entry_t *mock_find(const char *key, char type)
{
    for (size_t i = 0; i < g_entry_count; i++) {
        if (g_entries[i].type == type && strcmp(g_entries[i].key, key) == 0) {
            return &g_entries[i];
        }
    }
    return NULL;
}

static void mock_record(const char *key)
{
    if (g_query_count < sizeof(g_queries) / sizeof(g_queries[0])) {
        snprintf(g_queries[g_query_count], sizeof(g_queries[0]), "%s", key);
        g_query_count++;
    }
}

static int mock_get_str(void *ctx, const char *key, char *out, size_t cap)
{
    (void)ctx;
    mock_record(key);
    const entry_t *e = mock_find(key, 's');
    if (e == NULL) {
        return 0;
    }
    if (e->rc != 1) {
        return e->rc;
    }
    if (strlen(e->sval) + 1 > cap) {
        return -1;
    }
    snprintf(out, cap, "%s", e->sval);
    return 1;
}

static int mock_get_i32(void *ctx, const char *key, int32_t *out)
{
    (void)ctx;
    mock_record(key);
    const entry_t *e = mock_find(key, 'i');
    if (e == NULL) {
        return 0;
    }
    if (e->rc != 1) {
        return e->rc;
    }
    *out = e->ival;
    return 1;
}

static int mock_get_f64(void *ctx, const char *key, double *out)
{
    (void)ctx;
    mock_record(key);
    const entry_t *e = mock_find(key, 'f');
    if (e == NULL) {
        return 0;
    }
    if (e->rc != 1) {
        return e->rc;
    }
    *out = e->fval;
    return 1;
}

static const clubins_config_overrides_t g_mock = {
    .ctx = NULL,
    .get_str = mock_get_str,
    .get_i32 = mock_get_i32,
    .get_f64 = mock_get_f64,
};

/* ------------------------------------------------------------------ */
/* Helpers                                                             */
/* ------------------------------------------------------------------ */

/** The warn list contains `key` with kind `kind`. */
static void assert_warned(const clubins_config_warn_t *warn, const char *key,
                          clubins_config_warn_kind_t kind)
{
    for (size_t i = 0; i < warn->count; i++) {
        if (strcmp(warn->entries[i].key, key) == 0 && warn->entries[i].kind == kind) {
            return;
        }
    }
    char message[128];
    snprintf(message, sizeof(message), "expected warn entry %s (%s) not found", key,
             clubins_config_warn_kind_name(kind));
    TEST_FAIL_MESSAGE(message);
}

/** The warn list does NOT contain `key`. */
static void assert_not_warned(const clubins_config_warn_t *warn, const char *key)
{
    for (size_t i = 0; i < warn->count; i++) {
        if (strcmp(warn->entries[i].key, key) == 0) {
            char message[128];
            snprintf(message, sizeof(message), "unexpected warn entry %s", key);
            TEST_FAIL_MESSAGE(message);
        }
    }
}

static char toml_buf[4096];
static char dump_buf[4096];

/** Read the committed config file (the real default.toml, not a copy). */
static const char *read_committed_default_toml(void)
{
    FILE *f = fopen(CLUBINS_DEFAULT_TOML_PATH, "rb");
    TEST_ASSERT_NOT_NULL_MESSAGE(f, "committed config/default.toml must exist");
    size_t n = fread(toml_buf, 1, sizeof(toml_buf) - 1, f);
    fclose(f);
    toml_buf[n] = '\0';
    return toml_buf;
}

/* ------------------------------------------------------------------ */
/* 1. Precedence                                                       */
/* ------------------------------------------------------------------ */

void test_builtins_match_the_spec_example(void)
{
    clubins_config_t builtins;
    clubins_config_builtins(&builtins);
    TEST_ASSERT_EQUAL_STRING("00000000-0000-0000-0000-000000000000", builtins.sensor_id);
    TEST_ASSERT_EQUAL_INT(1000, builtins.decibel_window_ms);
    TEST_ASSERT_EQUAL_INT(60, builtins.report_interval_s);
    TEST_ASSERT_EQUAL_INT(64, builtins.offline_buffer);
    TEST_ASSERT_EQUAL_INT(0x76, builtins.bme280_i2c_address);
    TEST_ASSERT_EQUAL_INT(4, builtins.pir_gpio_pin);
    TEST_ASSERT_EQUAL_INT(CLUBINS_TRANSPORT_HTTP, builtins.transport_active);
    TEST_ASSERT_EQUAL_STRING("https://ingest.clubins.de/v1/sensors", builtins.http_endpoint);
    TEST_ASSERT_EQUAL_STRING("mqtt.clubins.de", builtins.mqtt_broker);
    TEST_ASSERT_EQUAL_INT(8883, builtins.mqtt_port);
    TEST_ASSERT_EQUAL_DOUBLE(120.0, builtins.overload_threshold_db);
    TEST_ASSERT_EQUAL_DOUBLE(0.0, builtins.calibration_offset_db);
}

void test_load_null_toml_and_null_overrides_yields_builtins(void)
{
    clubins_config_warn_t warn;
    clubins_config_t expected;
    clubins_config_builtins(&expected);
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK,
                      clubins_config_load(NULL, NULL, &warn));
    TEST_ASSERT_TRUE(clubins_config_equal(&expected, clubins_config_pool()));
    TEST_ASSERT_EQUAL_INT(0, (int)warn.count);
}

void test_file_tier_overrides_builtins(void)
{
    static const char *toml = "[sampling]\n"
                              "report_interval_s = 300\n"
                              "\n[decibel]\n"
                              "calibration_offset_db = 2.5\n";
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(toml, NULL, &warn));
    const clubins_config_t *pool = clubins_config_pool();
    TEST_ASSERT_EQUAL_INT(300, pool->report_interval_s);
    TEST_ASSERT_EQUAL_DOUBLE(2.5, pool->calibration_offset_db);
    /* Untouched fields keep their built-ins. */
    TEST_ASSERT_EQUAL_INT(1000, pool->decibel_window_ms);
    TEST_ASSERT_EQUAL_STRING("https://ingest.clubins.de/v1/sensors", pool->http_endpoint);
    TEST_ASSERT_EQUAL_INT(0, (int)warn.count);
}

void test_override_tier_beats_file_tier(void)
{
    static const char *toml = "[sampling]\n"
                              "report_interval_s = 300\n";
    mock_reset();
    entry_t report = {.key = "sampling.report_interval_s", .type = 'i', .ival = 15, .rc = 1};
    entry_t cal = {.key = "decibel.calibration_offset_db", .type = 'f', .fval = -1.75, .rc = 1};
    mock_add(report);
    mock_add(cal);
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(toml, &g_mock, &warn));
    TEST_ASSERT_EQUAL_INT(15, clubins_config_pool()->report_interval_s);
    TEST_ASSERT_EQUAL_DOUBLE(-1.75, clubins_config_pool()->calibration_offset_db);
    assert_not_warned(&warn, "sampling.report_interval_s");
}

void test_override_tier_beats_builtins_without_file(void)
{
    mock_reset();
    entry_t port = {.key = "transport.mqtt.port", .type = 'i', .ival = 1883, .rc = 1};
    entry_t broker = {.key = "transport.mqtt.broker", .type = 's', .sval = "10.0.0.9", .rc = 1};
    mock_add(port);
    mock_add(broker);
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(NULL, &g_mock, &warn));
    const clubins_config_t *pool = clubins_config_pool();
    TEST_ASSERT_EQUAL_INT(1883, pool->mqtt_port);
    TEST_ASSERT_EQUAL_STRING("10.0.0.9", pool->mqtt_broker);
    /* Untouched built-ins stand. */
    TEST_ASSERT_EQUAL_INT(CLUBINS_TRANSPORT_HTTP, pool->transport_active);
    TEST_ASSERT_EQUAL_INT(0, (int)warn.count);
}

/* ------------------------------------------------------------------ */
/* 2. Unknown-key tolerance                                            */
/* ------------------------------------------------------------------ */

void test_unknown_keys_warn_and_do_not_apply(void)
{
    static const char *toml = "stray_root = 1\n"
                              "\n[sensor]\n"
                              "id = \"ab12cd34-5678-90ef-aabb-ccddeeff0011\"\n"
                              "typo = 3\n"
                              "\n[sensors]\n"
                              "sht4x = 5\n"
                              "\n[sampling]\n"
                              "report_interval_s = 120\n"
                              "cadence = 9\n";
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(toml, NULL, &warn));
    assert_warned(&warn, "stray_root", CLUBINS_CONFIG_WARN_UNKNOWN_KEY);
    assert_warned(&warn, "sensor.typo", CLUBINS_CONFIG_WARN_UNKNOWN_KEY);
    assert_warned(&warn, "sensors.sht4x", CLUBINS_CONFIG_WARN_UNKNOWN_KEY);
    assert_warned(&warn, "sampling.cadence", CLUBINS_CONFIG_WARN_UNKNOWN_KEY);
    TEST_ASSERT_EQUAL_INT(4, (int)warn.count);
    TEST_ASSERT_FALSE(warn.truncated);
    /* Known keys still applied; unknown ones never touched the pool. */
    const clubins_config_t *pool = clubins_config_pool();
    TEST_ASSERT_EQUAL_STRING("ab12cd34-5678-90ef-aabb-ccddeeff0011", pool->sensor_id);
    TEST_ASSERT_EQUAL_INT(120, pool->report_interval_s);
    TEST_ASSERT_EQUAL_INT(64, pool->offline_buffer);
}

void test_unknown_keys_beyond_capacity_set_truncated(void)
{
    char toml[1024];
    toml[0] = '\0';
    for (int i = 0; i < 20; i++) {
        snprintf(toml + strlen(toml), sizeof(toml) - strlen(toml), "unknown_key_%d = %d\n", i, i);
    }
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(toml, NULL, &warn));
    TEST_ASSERT_TRUE(warn.truncated);
    TEST_ASSERT_EQUAL_INT(CLUBINS_CONFIG_MAX_WARN, (int)warn.count);
}

void test_bad_values_warn_and_keep_lower_tier(void)
{
    /* File tier: out of range / wrong type / wrong vocabulary. */
    static const char *toml = "[sampling]\n"
                              "report_interval_s = 0\n"
                              "decibel_window_ms = \"fast\"\n"
                              "\n[sensors.bme280]\n"
                              "i2c_address = 0x01\n"
                              "\n[transport]\n"
                              "active = \"gopher\"\n"
                              "http_endpoint = \"not-a-url\"\n"
                              "\n[transport.mqtt]\n"
                              "port = 70000\n"
                              "\n[decibel]\n"
                              "overload_threshold_db = -1.0\n";
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(toml, NULL, &warn));
    assert_warned(&warn, "sampling.report_interval_s", CLUBINS_CONFIG_WARN_BAD_VALUE);
    assert_warned(&warn, "sampling.decibel_window_ms", CLUBINS_CONFIG_WARN_BAD_VALUE);
    assert_warned(&warn, "sensors.bme280.i2c_address", CLUBINS_CONFIG_WARN_BAD_VALUE);
    assert_warned(&warn, "transport.active", CLUBINS_CONFIG_WARN_BAD_VALUE);
    assert_warned(&warn, "transport.http_endpoint", CLUBINS_CONFIG_WARN_BAD_VALUE);
    assert_warned(&warn, "transport.mqtt.port", CLUBINS_CONFIG_WARN_BAD_VALUE);
    assert_warned(&warn, "decibel.overload_threshold_db", CLUBINS_CONFIG_WARN_BAD_VALUE);
    /* Every rejected value left its built-in standing. */
    const clubins_config_t *pool = clubins_config_pool();
    TEST_ASSERT_EQUAL_INT(60, pool->report_interval_s);
    TEST_ASSERT_EQUAL_INT(1000, pool->decibel_window_ms);
    TEST_ASSERT_EQUAL_INT(0x76, pool->bme280_i2c_address);
    TEST_ASSERT_EQUAL_INT(CLUBINS_TRANSPORT_HTTP, pool->transport_active);
    TEST_ASSERT_EQUAL_STRING("https://ingest.clubins.de/v1/sensors", pool->http_endpoint);
    TEST_ASSERT_EQUAL_INT(8883, pool->mqtt_port);
    TEST_ASSERT_EQUAL_DOUBLE(120.0, pool->overload_threshold_db);
}

void test_override_bad_value_keeps_file_tier_value(void)
{
    static const char *toml = "[sampling]\n"
                              "report_interval_s = 120\n";
    mock_reset();
    entry_t bad = {.key = "sampling.report_interval_s", .type = 'i', .ival = -5, .rc = 1};
    mock_add(bad);
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(toml, &g_mock, &warn));
    assert_warned(&warn, "sampling.report_interval_s", CLUBINS_CONFIG_WARN_BAD_VALUE);
    TEST_ASSERT_EQUAL_INT(120, clubins_config_pool()->report_interval_s);
}

void test_override_string_too_long_is_rejected_not_truncated(void)
{
    mock_reset();
    entry_t huge = {.key = "sensor.id", .type = 's', .sval = "x", .rc = 1};
    char long_id[64];
    memset(long_id, 'a', sizeof(long_id) - 1);
    long_id[sizeof(long_id) - 1] = '\0';
    huge.sval = long_id;
    mock_add(huge);
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(NULL, &g_mock, &warn));
    assert_warned(&warn, "sensor.id", CLUBINS_CONFIG_WARN_BAD_VALUE);
    TEST_ASSERT_EQUAL_STRING("00000000-0000-0000-0000-000000000000",
                             clubins_config_pool()->sensor_id);
}

void test_source_error_warns_and_keeps_lower_tier(void)
{
    static const char *toml = "[sampling]\n"
                              "report_interval_s = 120\n";
    mock_reset();
    entry_t error = {.key = "sampling.report_interval_s", .type = 'i', .rc = -1};
    mock_add(error);
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(toml, &g_mock, &warn));
    assert_warned(&warn, "sampling.report_interval_s", CLUBINS_CONFIG_WARN_SOURCE_ERROR);
    TEST_ASSERT_EQUAL_INT(120, clubins_config_pool()->report_interval_s);
}

void test_unparseable_file_keeps_builtins_and_overrides(void)
{
    static const char *toml = "[sampling report_interval_s = [unclosed";
    mock_reset();
    entry_t report = {.key = "sampling.report_interval_s", .type = 'i', .ival = 15, .rc = 1};
    mock_add(report);
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_ERR_TOML, clubins_config_load(toml, &g_mock, &warn));
    assert_warned(&warn, "<file>", CLUBINS_CONFIG_WARN_BAD_VALUE);
    /* Built-ins + overrides still produced a usable pool. */
    TEST_ASSERT_EQUAL_INT(15, clubins_config_pool()->report_interval_s);
    TEST_ASSERT_EQUAL_INT(1000, clubins_config_pool()->decibel_window_ms);
}

/* ------------------------------------------------------------------ */
/* 3. Secret exclusion                                                 */
/* ------------------------------------------------------------------ */

void test_loader_never_queries_secret_keys(void)
{
    mock_reset();
    entry_t report = {.key = "sampling.report_interval_s", .type = 'i', .ival = 15, .rc = 1};
    mock_add(report);
    clubins_config_warn_t warn;
    clubins_config_load(NULL, &g_mock, &warn);
    for (size_t i = 0; i < g_query_count; i++) {
        TEST_ASSERT_NULL_MESSAGE(
            strstr(g_queries[i], "apikey"), "loader queried an apikey key");
        TEST_ASSERT_NULL_MESSAGE(
            strstr(g_queries[i], "password"), "loader queried a password key");
        TEST_ASSERT_NULL_MESSAGE(
            strstr(g_queries[i], "secret"), "loader queried a secret key");
        TEST_ASSERT_NULL_MESSAGE(strstr(g_queries[i], "cert"), "loader queried a cert key");
    }
}

void test_dump_contains_no_secret_fields(void)
{
    /* Hostile file AND hostile override: secret-shaped keys everywhere the
     * schema will not recognize them. */
    static const char *toml = "[api]\n"
                              "apikey = \"sk-supersecret-123\"\n"
                              "\n[wifi]\n"
                              "ssid = \"venue-guest\"\n"
                              "password = \"hunter2-hunter2\"\n"
                              "\n[transport]\n"
                              "active = \"http\"\n"
                              "apikey = \"sk-in-transport\"\n";
    mock_reset();
    entry_t secret = {.key = "wifi.password", .type = 's', .sval = "nvs-secret-value", .rc = 1};
    mock_add(secret);
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(toml, &g_mock, &warn));
    /* The secrets were refused as unknown keys / unqueried overrides. */
    assert_warned(&warn, "api", CLUBINS_CONFIG_WARN_UNKNOWN_KEY);
    assert_warned(&warn, "wifi", CLUBINS_CONFIG_WARN_UNKNOWN_KEY);
    assert_warned(&warn, "transport.apikey", CLUBINS_CONFIG_WARN_UNKNOWN_KEY);
    TEST_ASSERT_TRUE(clubins_config_dump(clubins_config_pool(), dump_buf, sizeof(dump_buf)) > 0);
    TEST_ASSERT_NULL(strstr(dump_buf, "apikey"));
    TEST_ASSERT_NULL(strstr(dump_buf, "password"));
    TEST_ASSERT_NULL(strstr(dump_buf, "secret"));
    TEST_ASSERT_NULL(strstr(dump_buf, "sk-supersecret"));
    TEST_ASSERT_NULL(strstr(dump_buf, "hunter2"));
    TEST_ASSERT_NULL(strstr(dump_buf, "BEGIN"));
}

void test_committed_default_toml_has_no_secret_fields(void)
{
    const char *text = read_committed_default_toml();
    TEST_ASSERT_NULL(strstr(text, "apikey"));
    TEST_ASSERT_NULL(strstr(text, "password"));
    TEST_ASSERT_NULL(strstr(text, "secret"));
    TEST_ASSERT_NULL(strstr(text, "psk"));
    TEST_ASSERT_NULL(strstr(text, "token"));
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(text, NULL, &warn));
    TEST_ASSERT_EQUAL_INT(0, (int)warn.count);
}

/* ------------------------------------------------------------------ */
/* 4. Round-trip parse                                                 */
/* ------------------------------------------------------------------ */

void test_dump_round_trips_the_committed_file(void)
{
    const char *text = read_committed_default_toml();
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(text, NULL, &warn));
    clubins_config_t original = *clubins_config_pool();
    TEST_ASSERT_TRUE(clubins_config_dump(&original, dump_buf, sizeof(dump_buf)) > 0);

    /* The dump itself must parse as TOML and rebuild the same pool. */
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(dump_buf, NULL, &warn));
    TEST_ASSERT_TRUE(clubins_config_equal(&original, clubins_config_pool()));
    /* Spot-check that the dump carries real values, not blanks. */
    TEST_ASSERT_EQUAL_INT(8883, clubins_config_pool()->mqtt_port);
    TEST_ASSERT_EQUAL_DOUBLE(120.0, clubins_config_pool()->overload_threshold_db);
}

void test_dump_round_trips_a_mutated_pool_exactly(void)
{
    clubins_config_t mutated;
    clubins_config_builtins(&mutated);
    /* Values chosen to exercise the double rendering (fractional, no
     * trailing fraction) and the string/enum paths. */
    snprintf(mutated.sensor_id, sizeof(mutated.sensor_id), "%s",
             "11223344-5566-7788-aabb-ccddeeff0011");
    mutated.report_interval_s = 300;
    mutated.offline_buffer = 7;
    mutated.bme280_i2c_address = 0x77;
    mutated.pir_gpio_pin = 13;
    mutated.transport_active = CLUBINS_TRANSPORT_MQTT;
    snprintf(mutated.http_endpoint, sizeof(mutated.http_endpoint), "%s",
             "http://localhost:4000/v1/sensors");
    snprintf(mutated.mqtt_broker, sizeof(mutated.mqtt_broker), "%s", "10.0.0.9");
    mutated.mqtt_port = 1883;
    mutated.overload_threshold_db = 121.5;
    mutated.calibration_offset_db = -3.25;

    TEST_ASSERT_TRUE(clubins_config_dump(&mutated, dump_buf, sizeof(dump_buf)) > 0);
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(dump_buf, NULL, NULL));
    TEST_ASSERT_TRUE(clubins_config_equal(&mutated, clubins_config_pool()));
}

void test_dump_reports_truncation_instead_of_lying(void)
{
    clubins_config_t builtins;
    clubins_config_builtins(&builtins);
    TEST_ASSERT_EQUAL(-1, clubins_config_dump(&builtins, dump_buf, 8));
    TEST_ASSERT_TRUE(strlen(dump_buf) < 8);
}

void test_pool_accessor_serves_the_loaded_instance(void)
{
    /* Tests above loaded the pool many times; the accessor must serve the
     * latest load, not a stale copy. */
    clubins_config_warn_t warn;
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(NULL, NULL, &warn));
    const clubins_config_t *first = clubins_config_pool();
    TEST_ASSERT_NOT_NULL(first);
    TEST_ASSERT_EQUAL_INT(60, first->report_interval_s);
    static const char *toml = "[sampling]\n"
                              "report_interval_s = 45\n";
    TEST_ASSERT_EQUAL(CLUBINS_CONFIG_OK, clubins_config_load(toml, NULL, &warn));
    TEST_ASSERT_EQUAL_PTR(first, clubins_config_pool());
    TEST_ASSERT_EQUAL_INT(45, clubins_config_pool()->report_interval_s);
}

/* ------------------------------------------------------------------ */

int main(void)
{
    UNITY_BEGIN();
    /* 1. precedence */
    RUN_TEST(test_builtins_match_the_spec_example);
    RUN_TEST(test_load_null_toml_and_null_overrides_yields_builtins);
    RUN_TEST(test_file_tier_overrides_builtins);
    RUN_TEST(test_override_tier_beats_file_tier);
    RUN_TEST(test_override_tier_beats_builtins_without_file);
    /* 2. unknown-key tolerance */
    RUN_TEST(test_unknown_keys_warn_and_do_not_apply);
    RUN_TEST(test_unknown_keys_beyond_capacity_set_truncated);
    RUN_TEST(test_bad_values_warn_and_keep_lower_tier);
    RUN_TEST(test_override_bad_value_keeps_file_tier_value);
    RUN_TEST(test_override_string_too_long_is_rejected_not_truncated);
    RUN_TEST(test_source_error_warns_and_keeps_lower_tier);
    RUN_TEST(test_unparseable_file_keeps_builtins_and_overrides);
    /* 3. secret exclusion */
    RUN_TEST(test_loader_never_queries_secret_keys);
    RUN_TEST(test_dump_contains_no_secret_fields);
    RUN_TEST(test_committed_default_toml_has_no_secret_fields);
    /* 4. round-trip */
    RUN_TEST(test_dump_round_trips_the_committed_file);
    RUN_TEST(test_dump_round_trips_a_mutated_pool_exactly);
    RUN_TEST(test_dump_reports_truncation_instead_of_lying);
    RUN_TEST(test_pool_accessor_serves_the_loaded_instance);
    return UNITY_END();
}
