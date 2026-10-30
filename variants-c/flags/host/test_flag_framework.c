//! Port of V1's tests/flag_framework.rs — CTest + Unity.
//!
//! - feature_flag_resolution: the registry must mirror the Kconfig
//!   capability selection of this build exactly, so a mis-wired flag fails
//!   in the combination that broke it. Runs under every matrix combination.
//! - module_presence: every flag-toggled stub is addressable in this build
//!   (unique cap, contract-exact module marker), disabled = absent, and
//!   the sensor wire types match the ingest contracts exactly.
//! - stub_lifecycle: init/tick are wired for every compiled-in module and
//!   safe to drive repeatedly — the same calls the device's FreeRTOS timer
//!   makes.

#include "cap_registry.h"
#include "caps_config.h"

#include "unity.h"

void setUp(void)
{
}

void tearDown(void)
{
}

/** The expected resolution, computed here from the same CONFIG_CLUBINS_CAP_*
 * definitions the build was invoked with — the port of V1's cfg!() test
 * side, deliberately independent of the registry's own switch so a
 * mis-wired arm on either side fails. */
static bool test_side_enabled(clubins_cap_id_t cap)
{
    switch (cap) {
    case CLUBINS_CAP_SENSOR_DECIBEL:
#if CONFIG_CLUBINS_CAP_SENSOR_DECIBEL
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_SENSOR_PAX:
#if CONFIG_CLUBINS_CAP_SENSOR_PAX
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_SENSOR_BME280:
#if CONFIG_CLUBINS_CAP_SENSOR_BME280
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_SENSOR_SHT4X:
#if CONFIG_CLUBINS_CAP_SENSOR_SHT4X
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_SENSOR_SCD4X:
#if CONFIG_CLUBINS_CAP_SENSOR_SCD4X
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_SENSOR_PMS5003:
#if CONFIG_CLUBINS_CAP_SENSOR_PMS5003
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_SENSOR_BH1750:
#if CONFIG_CLUBINS_CAP_SENSOR_BH1750
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_TRANSPORT_HTTP:
#if CONFIG_CLUBINS_CAP_TRANSPORT_HTTP
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_TRANSPORT_MQTT:
#if CONFIG_CLUBINS_CAP_TRANSPORT_MQTT
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_TRANSPORT_MTLS:
#if CONFIG_CLUBINS_CAP_TRANSPORT_MTLS
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_OTA:
#if CONFIG_CLUBINS_CAP_OTA
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_PROVISIONING:
#if CONFIG_CLUBINS_CAP_PROVISIONING
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_DEEP_SLEEP:
#if CONFIG_CLUBINS_CAP_DEEP_SLEEP
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_LOGGING_DEFMT:
#if CONFIG_CLUBINS_CAP_LOGGING_DEFMT
        return true;
#else
        return false;
#endif
    case CLUBINS_CAP_OPS_STATUS:
#if CONFIG_CLUBINS_CAP_OPS_STATUS
        return true;
#else
        return false;
#endif
    default:
        return false;
    }
}

/** Contract-exact ingest types (spec §Sensor Expansion; the strings were
 * verified against the Go ingest structs and api/mqtt/topics.md in the
 * source monorepo). */
static const char *test_side_ingest_type(clubins_cap_id_t cap)
{
    switch (cap) {
    case CLUBINS_CAP_SENSOR_DECIBEL:
        return "decibel";
    case CLUBINS_CAP_SENSOR_PAX:
        return "pax";
    case CLUBINS_CAP_SENSOR_BME280:
    case CLUBINS_CAP_SENSOR_SHT4X:
        return "weather";
    case CLUBINS_CAP_SENSOR_SCD4X:
        return "co2";
    case CLUBINS_CAP_SENSOR_PMS5003:
        return "pm25";
    case CLUBINS_CAP_SENSOR_BH1750:
        return "light";
    default:
        return NULL;
    }
}

/** Feature-flag resolution: the compiled-in sensor set must exactly match
 * the Kconfig capability selection of this build. */
static void test_feature_flag_resolution(void)
{
    clubins_sensor_meta_t expected[CLUBINS_CAP_MAX_SENSORS];
    size_t expected_count = 0;

    for (int i = 0; i < CLUBINS_CAP_COUNT; i++) {
        const clubins_cap_id_t cap = (clubins_cap_id_t)i;
        if (clubins_sensor_ingest_type(cap) == NULL) {
            continue; /* not a sensor family */
        }
        TEST_ASSERT_EQUAL_MESSAGE(test_side_enabled(cap), clubins_cap_enabled(cap),
                                  "resolution arm disagrees with the Kconfig selection");
        if (!test_side_enabled(cap)) {
            continue;
        }
        TEST_ASSERT_TRUE(expected_count < CLUBINS_CAP_MAX_SENSORS);
        expected[expected_count].cap = cap;
        expected[expected_count].ingest_type = clubins_sensor_ingest_type(cap);
        expected_count++;
    }

    clubins_sensor_meta_t metas[CLUBINS_CAP_MAX_SENSORS];
    const size_t count = clubins_enabled_sensor_metas(metas, CLUBINS_CAP_MAX_SENSORS);
    TEST_ASSERT_EQUAL_MESSAGE(expected_count, count,
                              "registry resolution must mirror the Kconfig capability selection");

    for (size_t i = 0; i < count; i++) {
        TEST_ASSERT_EQUAL(expected[i].cap, metas[i].cap);
        TEST_ASSERT_NOT_NULL(metas[i].ingest_type);
        TEST_ASSERT_EQUAL_STRING(expected[i].ingest_type, metas[i].ingest_type);
        TEST_ASSERT_EQUAL_STRING(clubins_sensor_ingest_type(metas[i].cap), metas[i].ingest_type);
        TEST_ASSERT_TRUE(clubins_cap_enabled(metas[i].cap));
        TEST_ASSERT_NOT_NULL(clubins_cap_name(metas[i].cap));
    }
}

/** Module presence: exactly the enabled capabilities appear in the
 * registry, each with a unique id and its identity-table marker; disabled
 * = absent. Anchor sensor wire names are asserted against the contracts. */
static void test_module_presence(void)
{
    const clubins_module_t *modules[CLUBINS_CAP_MAX_MODULES];
    const size_t count = clubins_registry_modules(modules, CLUBINS_CAP_MAX_MODULES);

    size_t enabled_count = 0;
    for (int i = 0; i < CLUBINS_CAP_COUNT; i++) {
        if (test_side_enabled((clubins_cap_id_t)i)) {
            enabled_count++;
        }
    }
    TEST_ASSERT_EQUAL_MESSAGE(enabled_count, count,
                              "the registry must list exactly the enabled capabilities");

    for (size_t i = 0; i < count; i++) {
        TEST_ASSERT_TRUE(clubins_cap_enabled(modules[i]->cap));
        TEST_ASSERT_EQUAL_STRING(clubins_cap_name(modules[i]->cap), modules[i]->name);
        if (clubins_sensor_ingest_type(modules[i]->cap) != NULL) {
            TEST_ASSERT_EQUAL_STRING(clubins_sensor_ingest_type(modules[i]->cap),
                                     modules[i]->ingest_type);
        } else {
            TEST_ASSERT_NULL(modules[i]->ingest_type);
        }
        for (size_t j = i + 1; j < count; j++) {
            TEST_ASSERT_NOT_EQUAL(modules[i]->cap, modules[j]->cap);
        }
    }

    /* Anchor wire names match the ingest contracts exactly (the port of
     * V1's SensorKind::ingest_type assertions). */
    TEST_ASSERT_EQUAL_STRING("decibel", clubins_sensor_ingest_type(CLUBINS_CAP_SENSOR_DECIBEL));
    TEST_ASSERT_EQUAL_STRING("pax", clubins_sensor_ingest_type(CLUBINS_CAP_SENSOR_PAX));
    TEST_ASSERT_EQUAL_STRING("weather", clubins_sensor_ingest_type(CLUBINS_CAP_SENSOR_BME280));
    TEST_ASSERT_EQUAL_STRING("weather", clubins_sensor_ingest_type(CLUBINS_CAP_SENSOR_SHT4X));
    TEST_ASSERT_EQUAL_STRING("co2", clubins_sensor_ingest_type(CLUBINS_CAP_SENSOR_SCD4X));
    TEST_ASSERT_EQUAL_STRING("pm25", clubins_sensor_ingest_type(CLUBINS_CAP_SENSOR_PMS5003));
    TEST_ASSERT_EQUAL_STRING("light", clubins_sensor_ingest_type(CLUBINS_CAP_SENSOR_BH1750));
}

/** Stub lifecycle: the hooks the device's FreeRTOS timer drives are wired
 * for every compiled-in module and safe to call repeatedly. */
static void test_stub_lifecycle(void)
{
    const clubins_module_t *modules[CLUBINS_CAP_MAX_MODULES];
    const size_t count = clubins_registry_modules(modules, CLUBINS_CAP_MAX_MODULES);

    clubins_registry_init_all();
    clubins_registry_tick_all();
    clubins_registry_tick_all();

    for (size_t i = 0; i < count; i++) {
        TEST_ASSERT_NOT_NULL(modules[i]->init);
        TEST_ASSERT_NOT_NULL(modules[i]->tick);
    }
}

int main(void)
{
    UNITY_BEGIN();
    RUN_TEST(test_feature_flag_resolution);
    RUN_TEST(test_module_presence);
    RUN_TEST(test_stub_lifecycle);
    return UNITY_END();
}
