//! Capability registry implementation — the C port of V1's registry.rs.
//!
//! The compile-time half of the flag framework: the authoritative mapping
//! from Kconfig capability selection to the module set a build carries.
//! Each arm is a compile-time constant (via CONFIG_CLUBINS_CAP_* from
//! caps_config.h), so a build without a flag carries none of its driver
//! code — disabled = absent, exactly V1's semantics.

#include "cap_registry.h"

#include "caps_config.h"
#include "stubkit.h"

#include <string.h>

/** Module markers for every capability, always addressable (the port of
 * V1's `SensorKind::ALL` + `MODULE` consts). */
static const char *const s_cap_names[CLUBINS_CAP_COUNT] = {
    [CLUBINS_CAP_SENSOR_DECIBEL] = "sensors::pdm_mic",
    [CLUBINS_CAP_SENSOR_PAX] = "sensors::pir",
    [CLUBINS_CAP_SENSOR_BME280] = "sensors::bme280",
    [CLUBINS_CAP_SENSOR_SHT4X] = "sensors::sht4x",
    [CLUBINS_CAP_SENSOR_SCD4X] = "sensors::scd4x",
    [CLUBINS_CAP_SENSOR_PMS5003] = "sensors::pms5003",
    [CLUBINS_CAP_SENSOR_BH1750] = "sensors::bh1750",
    [CLUBINS_CAP_TRANSPORT_HTTP] = "transports::http",
    [CLUBINS_CAP_TRANSPORT_MQTT] = "transports::mqtt",
    [CLUBINS_CAP_TRANSPORT_MTLS] = "ops::mtls",
    [CLUBINS_CAP_OTA] = "ops::ota",
    [CLUBINS_CAP_PROVISIONING] = "ops::ble_provisioning",
    [CLUBINS_CAP_DEEP_SLEEP] = "ops::deep_sleep",
    [CLUBINS_CAP_LOGGING_DEFMT] = "ops::logging_defmt",
    [CLUBINS_CAP_OPS_STATUS] = "ops::status",
};

const char *clubins_cap_name(clubins_cap_id_t cap)
{
    /* size_t cast: out-of-range or negative ids wrap into the >= check. */
    if ((size_t)cap >= CLUBINS_CAP_COUNT) {
        return NULL;
    }
    return s_cap_names[cap];
}

/** The ingest measurement type each sensor family feeds (the port of V1's
 * `SensorKind::ingest_type`). NULL for non-sensor capabilities. */
const char *clubins_sensor_ingest_type(clubins_cap_id_t cap)
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

/** Compile-time resolution, one arm per leaf — the port of V1's
 * `cfg!(feature = ...)` registry arms. A mis-wired arm fails the host
 * flag-resolution test in the combination that broke it. */
bool clubins_cap_enabled(clubins_cap_id_t cap)
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

/** The compiled-in module set, assembled at compile time from the Kconfig
 * selection. The trailing sentinel keeps the array non-empty when no
 * capability is enabled (the all-off build must compile); it is never
 * counted. */
static const clubins_module_t s_modules[] = {
#if CONFIG_CLUBINS_CAP_SENSOR_DECIBEL
    {CLUBINS_CAP_SENSOR_DECIBEL, "sensors::pdm_mic", "decibel", clubins_stub_pdm_mic_init,
     clubins_stub_pdm_mic_tick},
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PAX
    {CLUBINS_CAP_SENSOR_PAX, "sensors::pir", "pax", clubins_stub_pir_init, clubins_stub_pir_tick},
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BME280
    {CLUBINS_CAP_SENSOR_BME280, "sensors::bme280", "weather", clubins_stub_bme280_init,
     clubins_stub_bme280_tick},
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SHT4X
    {CLUBINS_CAP_SENSOR_SHT4X, "sensors::sht4x", "weather", clubins_stub_sht4x_init,
     clubins_stub_sht4x_tick},
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SCD4X
    {CLUBINS_CAP_SENSOR_SCD4X, "sensors::scd4x", "co2", clubins_stub_scd4x_init,
     clubins_stub_scd4x_tick},
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PMS5003
    {CLUBINS_CAP_SENSOR_PMS5003, "sensors::pms5003", "pm25", clubins_stub_pms5003_init,
     clubins_stub_pms5003_tick},
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BH1750
    {CLUBINS_CAP_SENSOR_BH1750, "sensors::bh1750", "light", clubins_stub_bh1750_init,
     clubins_stub_bh1750_tick},
#endif
#if CONFIG_CLUBINS_CAP_TRANSPORT_HTTP
    {CLUBINS_CAP_TRANSPORT_HTTP, "transports::http", NULL, clubins_stub_http_init,
     clubins_stub_http_tick},
#endif
#if CONFIG_CLUBINS_CAP_TRANSPORT_MQTT
    {CLUBINS_CAP_TRANSPORT_MQTT, "transports::mqtt", NULL, clubins_stub_mqtt_init,
     clubins_stub_mqtt_tick},
#endif
#if CONFIG_CLUBINS_CAP_TRANSPORT_MTLS
    {CLUBINS_CAP_TRANSPORT_MTLS, "ops::mtls", NULL, clubins_stub_mtls_init, clubins_stub_mtls_tick},
#endif
#if CONFIG_CLUBINS_CAP_OTA
    {CLUBINS_CAP_OTA, "ops::ota", NULL, clubins_stub_ota_init, clubins_stub_ota_tick},
#endif
#if CONFIG_CLUBINS_CAP_PROVISIONING
    {CLUBINS_CAP_PROVISIONING, "ops::ble_provisioning", NULL, clubins_stub_ble_provisioning_init,
     clubins_stub_ble_provisioning_tick},
#endif
#if CONFIG_CLUBINS_CAP_DEEP_SLEEP
    {CLUBINS_CAP_DEEP_SLEEP, "ops::deep_sleep", NULL, clubins_stub_deep_sleep_init,
     clubins_stub_deep_sleep_tick},
#endif
#if CONFIG_CLUBINS_CAP_LOGGING_DEFMT
    {CLUBINS_CAP_LOGGING_DEFMT, "ops::logging_defmt", NULL, clubins_stub_logging_defmt_init,
     clubins_stub_logging_defmt_tick},
#endif
#if CONFIG_CLUBINS_CAP_OPS_STATUS
    {CLUBINS_CAP_OPS_STATUS, "ops::status", NULL, clubins_stub_status_init,
     clubins_stub_status_tick},
#endif
    {CLUBINS_CAP_NONE, NULL, NULL, NULL, NULL}, /* sentinel: never counted */
};

/** Real module count: the table minus its sentinel. */
static size_t module_count(void)
{
    return (sizeof(s_modules) / sizeof(s_modules[0])) - 1;
}

size_t clubins_registry_modules(const clubins_module_t **out, size_t max)
{
    if (out == NULL || max == 0) {
        return 0;
    }
    const size_t count = module_count();
    const size_t written = (count < max) ? count : max;
    for (size_t i = 0; i < written; i++) {
        out[i] = &s_modules[i];
    }
    return written;
}

size_t clubins_enabled_sensor_metas(clubins_sensor_meta_t *out, size_t max)
{
    if (out == NULL || max == 0) {
        return 0;
    }
    size_t written = 0;
    const size_t count = module_count();
    for (size_t i = 0; i < count && written < max; i++) {
        if (s_modules[i].ingest_type == NULL) {
            continue; /* transports and ops are not sensor families */
        }
        out[written].cap = s_modules[i].cap;
        out[written].ingest_type = s_modules[i].ingest_type;
        written++;
    }
    return written;
}

void clubins_registry_init_all(void)
{
    const size_t count = module_count();
    for (size_t i = 0; i < count; i++) {
        if (s_modules[i].init != NULL) {
            s_modules[i].init();
        }
    }
}

void clubins_registry_tick_all(void)
{
    const size_t count = module_count();
    for (size_t i = 0; i < count; i++) {
        if (s_modules[i].tick != NULL) {
            s_modules[i].tick();
        }
    }
}
