//! Host tests for the registry + composition root — Unity.
//!
//! The registry is tested against a fake sensor pair; the composition is
//! tested against whatever the build's CONFIG_CLUBINS_CAP_* selection
//! composed: the tests derive their expectations from the same flags the
//! composition read, so this one binary verifies all 17 matrix
//! combinations (all-off composes nothing and must still pass).

#include "app_compose.h"
#include "registry.h"
#include "sensor.h"

#include <string.h>

#include "caps_config.h"
#include "unity.h"

void setUp(void)
{
}

void tearDown(void)
{
}

/* --- fake sensors for the registry tests ------------------------------- */

static clubins_sensor_status_t fake_init_ok(clubins_sensor_t *self)
{
    (void)self;
    return CLUBINS_SENSOR_OK;
}

static int g_init_order[CLUBINS_REGISTRY_MAX_SENSORS];
static size_t g_init_calls;

static clubins_sensor_status_t fake_init_record(clubins_sensor_t *self)
{
    g_init_order[g_init_calls++] = *(int *)self->ctx;
    return CLUBINS_SENSOR_OK;
}

static bool fake_sample_none(clubins_sensor_t *self, clubins_measurement_t *out)
{
    (void)self;
    (void)out;
    return false;
}

static const clubins_sensor_meta_t FAKE_META_A = {"fake-a", "fake-a", "u"};
static const clubins_sensor_meta_t FAKE_META_B = {"fake-b", "fake-b", "u"};

static const clubins_sensor_ops_t FAKE_OPS_A = {&FAKE_META_A, fake_init_ok, fake_sample_none};
static const clubins_sensor_ops_t FAKE_OPS_B = {&FAKE_META_B, fake_init_ok, fake_sample_none};
static const clubins_sensor_ops_t FAKE_OPS_NO_INIT = {&FAKE_META_A, NULL, fake_sample_none};
static const clubins_sensor_ops_t FAKE_OPS_RECORD = {&FAKE_META_A, fake_init_record,
                                                     fake_sample_none};

static void test_registry_starts_empty(void)
{
    clubins_registry_t registry;
    clubins_registry_init(&registry);
    TEST_ASSERT_TRUE(clubins_registry_is_empty(&registry));
    TEST_ASSERT_EQUAL_INT(0, clubins_registry_len(&registry));
    TEST_ASSERT_NULL(clubins_registry_at(&registry, 0));
}

static void test_registry_push_and_read_back(void)
{
    clubins_registry_t registry;
    clubins_registry_init(&registry);
    int ctx_a = 1;
    int ctx_b = 2;
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_OK,
                          clubins_registry_push(&registry, &FAKE_OPS_A, &ctx_a));
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_OK,
                          clubins_registry_push(&registry, &FAKE_OPS_B, &ctx_b));
    TEST_ASSERT_EQUAL_INT(2, clubins_registry_len(&registry));
    TEST_ASSERT_FALSE(clubins_registry_is_empty(&registry));

    const clubins_sensor_t *first = clubins_registry_at(&registry, 0);
    const clubins_sensor_t *second = clubins_registry_at(&registry, 1);
    TEST_ASSERT_NOT_NULL(first);
    TEST_ASSERT_NOT_NULL(second);
    TEST_ASSERT_EQUAL_PTR(&FAKE_OPS_A, first->ops);
    TEST_ASSERT_EQUAL_PTR(&ctx_a, first->ctx);
    TEST_ASSERT_EQUAL_STRING("fake-a", first->ops->meta->name);
    TEST_ASSERT_EQUAL_PTR(&FAKE_OPS_B, second->ops);
    TEST_ASSERT_NULL(clubins_registry_at(&registry, 2));
}

static void test_registry_push_rejects_bad_arguments(void)
{
    clubins_registry_t registry;
    clubins_registry_init(&registry);
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_ERR_BAD_ARGUMENT,
                          clubins_registry_push(NULL, &FAKE_OPS_A, NULL));
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_ERR_BAD_ARGUMENT,
                          clubins_registry_push(&registry, NULL, NULL));
    TEST_ASSERT_TRUE(clubins_registry_is_empty(&registry));
}

static void test_registry_push_fails_loudly_when_full(void)
{
    clubins_registry_t registry;
    clubins_registry_init(&registry);
    for (size_t i = 0; i < CLUBINS_REGISTRY_MAX_SENSORS; i++) {
        TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_OK,
                              clubins_registry_push(&registry, &FAKE_OPS_A, NULL));
    }
    /* One past capacity must fail loudly — a dropped driver silently loses
     * a measurement stream (V2's Fault("registry full")). */
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_ERR_FULL,
                          clubins_registry_push(&registry, &FAKE_OPS_B, NULL));
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_MAX_SENSORS, clubins_registry_len(&registry));
}

static void test_registry_init_all_walks_registration_order(void)
{
    clubins_registry_t registry;
    clubins_registry_init(&registry);
    int ctx_a = 10;
    int ctx_b = 20;
    int ctx_c = 30;
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_OK,
                          clubins_registry_push(&registry, &FAKE_OPS_RECORD, &ctx_a));
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_OK,
                          clubins_registry_push(&registry, &FAKE_OPS_RECORD, &ctx_b));
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_OK,
                          clubins_registry_push(&registry, &FAKE_OPS_RECORD, &ctx_c));
    g_init_calls = 0;
    clubins_registry_init_all(&registry);
    TEST_ASSERT_EQUAL_INT(3, g_init_calls);
    TEST_ASSERT_EQUAL_INT(10, g_init_order[0]);
    TEST_ASSERT_EQUAL_INT(20, g_init_order[1]);
    TEST_ASSERT_EQUAL_INT(30, g_init_order[2]);
}

static clubins_measurement_t g_sink_measurements[8];
static size_t g_sink_count;

static void capture_sink(void *sink_ctx, const clubins_measurement_t *measurement)
{
    (void)sink_ctx;
    if (g_sink_count < 8) {
        g_sink_measurements[g_sink_count++] = *measurement;
    }
}

static int g_fake_values[8];
static size_t g_fake_value_count;

static bool fake_sample_yield(clubins_sensor_t *self, clubins_measurement_t *out)
{
    (void)self;
    if (g_fake_value_count == 0) {
        return false;
    }
    out->type = CLUBINS_MEAS_PAX;
    out->as.pax.occupancy_count = (uint8_t)g_fake_values[0];
    g_fake_value_count--;
    return true;
}

static const clubins_sensor_ops_t FAKE_OPS_YIELD = {&FAKE_META_B, fake_init_ok, fake_sample_yield};

static void test_registry_sample_all_sinks_only_fresh_measurements(void)
{
    clubins_registry_t registry;
    clubins_registry_init(&registry);
    int quiet = 1;
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_OK,
                          clubins_registry_push(&registry, &FAKE_OPS_A, &quiet));

    g_sink_count = 0;
    g_fake_values[0] = 7;
    g_fake_value_count = 1;
    int talker = 2;
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_OK,
                          clubins_registry_push(&registry, &FAKE_OPS_YIELD, &talker));
    TEST_ASSERT_EQUAL_INT(CLUBINS_REGISTRY_OK,
                          clubins_registry_push(&registry, &FAKE_OPS_NO_INIT, &quiet));

    clubins_registry_sample_all(&registry, capture_sink, NULL);
    TEST_ASSERT_EQUAL_INT(1, g_sink_count);
    TEST_ASSERT_EQUAL_UINT8(CLUBINS_MEAS_PAX, g_sink_measurements[0].type);
    TEST_ASSERT_EQUAL_UINT8(7, g_sink_measurements[0].as.pax.occupancy_count);
}

static void test_registry_sample_all_tolerates_null_sink(void)
{
    clubins_registry_t registry;
    clubins_registry_init(&registry);
    clubins_registry_sample_all(&registry, NULL, NULL); /* must not crash */
}

/* --- composition tests: expectations derived from the same flags ------- */

static size_t expected_sensor_count(void)
{
    size_t count = 0;
#if CONFIG_CLUBINS_CAP_SENSOR_DECIBEL
    count++;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PAX
    count++;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BME280
    count++;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SHT4X
    count++;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SCD4X
    count++;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PMS5003
    count++;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BH1750
    count++;
#endif
    return count;
}

static size_t expected_hook_count(void)
{
    size_t count = 0;
#if CONFIG_CLUBINS_CAP_TRANSPORT_HTTP
    count++;
#endif
#if CONFIG_CLUBINS_CAP_TRANSPORT_MQTT
    count++;
#endif
#if CONFIG_CLUBINS_CAP_TRANSPORT_MTLS
    count++;
#endif
#if CONFIG_CLUBINS_CAP_OTA
    count++;
#endif
#if CONFIG_CLUBINS_CAP_PROVISIONING
    count++;
#endif
#if CONFIG_CLUBINS_CAP_DEEP_SLEEP
    count++;
#endif
#if CONFIG_CLUBINS_CAP_LOGGING_DEFMT
    count++;
#endif
#if CONFIG_CLUBINS_CAP_OPS_STATUS
    count++;
#endif
    return count;
}

/** The driver names the composition must have registered, in stable
 * registration order — built from the same flags the composition read. */
static const char *const EXPECTED_SENSOR_NAMES[] = {
#if CONFIG_CLUBINS_CAP_SENSOR_DECIBEL
    "SPK0641HT4H-1",
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PAX
    "PIR motion front-end",
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BME280
    "BME280",
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SHT4X
    "SHT4x",
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SCD4X
    "SCD4x",
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PMS5003
    "PMS5003",
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BH1750
    "BH1750",
#endif
};

static void test_compose_registers_exactly_the_flag_selected_sensors(void)
{
    clubins_app_compose();
    clubins_registry_t *registry = clubins_app_registry();
    TEST_ASSERT_NOT_NULL(registry);
    TEST_ASSERT_EQUAL_INT(expected_sensor_count(), clubins_app_composed_sensors());
    TEST_ASSERT_EQUAL_INT(expected_sensor_count(), clubins_registry_len(registry));
    for (size_t i = 0; i < expected_sensor_count(); i++) {
        const clubins_sensor_t *sensor = clubins_registry_at(registry, i);
        TEST_ASSERT_NOT_NULL(sensor);
        TEST_ASSERT_EQUAL_STRING(EXPECTED_SENSOR_NAMES[i], sensor->ops->meta->name);
        /* Every composed driver must carry static metadata. */
        TEST_ASSERT_NOT_NULL(sensor->ops->meta->ingest_type);
        TEST_ASSERT_NOT_NULL(sensor->ops->meta->unit);
    }
}

static void test_compose_is_idempotent(void)
{
    clubins_app_compose();
    size_t before = clubins_app_composed_sensors();
    clubins_app_compose();
    clubins_app_compose();
    TEST_ASSERT_EQUAL_INT(before, clubins_app_composed_sensors());
}

static void test_compose_hooks_match_flag_selection(void)
{
    clubins_app_compose();
    TEST_ASSERT_EQUAL_INT(expected_hook_count(), clubins_app_composed_hooks());
}

static void test_precompose_registry_is_null(void)
{
    /* Runs first in main(): before the first compose() call the
     * composition must not hand out a registry — it is built on demand,
     * not at load time. */
    TEST_ASSERT_NULL(clubins_app_registry());
    TEST_ASSERT_EQUAL_INT(0, clubins_app_composed_sensors());
    TEST_ASSERT_EQUAL_INT(0, clubins_app_composed_hooks());
}

#if CONFIG_CLUBINS_CAP_SENSOR_PAX
/** The PIR fold: motions recorded between ticks collapse into one pax
 * measurement, then the window resets (the take in V2's sample()). */
static void test_compose_pir_folds_and_resets_the_window(void)
{
    clubins_app_compose();
    clubins_app_init_all();

    clubins_registry_t *registry = clubins_app_registry();
    const clubins_sensor_t *pir = NULL;
    for (size_t i = 0; i < clubins_registry_len(registry); i++) {
        const clubins_sensor_t *sensor = clubins_registry_at(registry, i);
        if (strcmp(sensor->ops->meta->ingest_type, "pax") == 0) {
            pir = sensor;
            break;
        }
    }
    TEST_ASSERT_NOT_NULL(pir);

    uint32_t before = clubins_app_sampled_measurements();
    clubins_app_tick(); /* no motions recorded: nothing due */
    TEST_ASSERT_EQUAL_INT(before, clubins_app_sampled_measurements());

    clubins_pir_record_motion((clubins_pir_driver_t *)pir->ctx);
    clubins_pir_record_motion((clubins_pir_driver_t *)pir->ctx);
    clubins_pir_record_motion((clubins_pir_driver_t *)pir->ctx);
    clubins_app_tick(); /* three motions fold into one measurement */
    TEST_ASSERT_EQUAL_INT(before + 1, clubins_app_sampled_measurements());

    clubins_app_tick(); /* window reset: nothing due again */
    TEST_ASSERT_EQUAL_INT(before + 1, clubins_app_sampled_measurements());
}

/** Interrupt-side storms must clamp, not wrap, into the model's uint8
 * occupancy ceiling. */
static void test_compose_pir_clamps_at_model_ceiling(void)
{
    clubins_app_compose();
    clubins_app_init_all();

    clubins_registry_t *registry = clubins_app_registry();
    const clubins_sensor_t *pir = NULL;
    for (size_t i = 0; i < clubins_registry_len(registry); i++) {
        const clubins_sensor_t *sensor = clubins_registry_at(registry, i);
        if (strcmp(sensor->ops->meta->ingest_type, "pax") == 0) {
            pir = sensor;
            break;
        }
    }
    TEST_ASSERT_NOT_NULL(pir);

    clubins_pir_driver_t *driver = (clubins_pir_driver_t *)pir->ctx;
    for (uint32_t i = 0; i < 300; i++) {
        clubins_pir_record_motion(driver);
    }
    g_sink_count = 0;
    clubins_app_tick();
    /* The window's 300 motions clamp into the model's uint8 occupancy
     * ceiling (C1's fixed model decision) — no wrap, no truncation past
     * the wire maximum. */
    TEST_ASSERT_EQUAL_INT(1, g_sink_count);
    TEST_ASSERT_EQUAL_UINT8(255, g_sink_measurements[0].as.pax.occupancy_count);
    TEST_ASSERT_EQUAL_UINT32(0, driver->motions_since_sample);
}
#endif /* CONFIG_CLUBINS_CAP_SENSOR_PAX */

static void test_compose_tick_counts_composed_sampling(void)
{
    clubins_app_compose();
    clubins_app_init_all();
    uint32_t before = clubins_app_sampled_measurements();
    clubins_app_tick();
    /* Only the PIR driver yields on a fresh window — and only when
     * composed; every other scaffold driver samples nothing. */
#if CONFIG_CLUBINS_CAP_SENSOR_PAX
    TEST_ASSERT_EQUAL_INT(before + 1, clubins_app_sampled_measurements());
#else
    TEST_ASSERT_EQUAL_INT(before, clubins_app_sampled_measurements());
#endif
}

int main(void)
{
    UNITY_BEGIN();
    RUN_TEST(test_precompose_registry_is_null);
    RUN_TEST(test_registry_starts_empty);
    RUN_TEST(test_registry_push_and_read_back);
    RUN_TEST(test_registry_push_rejects_bad_arguments);
    RUN_TEST(test_registry_push_fails_loudly_when_full);
    RUN_TEST(test_registry_init_all_walks_registration_order);
    RUN_TEST(test_registry_sample_all_sinks_only_fresh_measurements);
    RUN_TEST(test_registry_sample_all_tolerates_null_sink);
    RUN_TEST(test_compose_registers_exactly_the_flag_selected_sensors);
    RUN_TEST(test_compose_is_idempotent);
    RUN_TEST(test_compose_hooks_match_flag_selection);
#if CONFIG_CLUBINS_CAP_SENSOR_PAX
    RUN_TEST(test_compose_pir_folds_and_resets_the_window);
    RUN_TEST(test_compose_pir_clamps_at_model_ceiling);
#endif
    RUN_TEST(test_compose_tick_counts_composed_sampling);
    return UNITY_END();
}
