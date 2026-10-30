//! Composition implementation — the port of V2's registry_from_features.
//!
//! Every capability arm is `#if`-guarded, so a build's composition is
//! exactly the set of components the Kconfig selection required. Disabled
//! = absent: no registry entry, no hook, no code in the link.

#include "app_compose.h"

#include <stdlib.h>

#include "caps_config.h"

#if CONFIG_CLUBINS_CAP_SENSOR_DECIBEL
#include "decibel_driver.h"
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PAX
#include "pir_driver.h"
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BME280
#include "bme280_driver.h"
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SHT4X
#include "sht4x_driver.h"
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SCD4X
#include "scd4x_driver.h"
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PMS5003
#include "pms5003_driver.h"
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BH1750
#include "bh1750_driver.h"
#endif
#if CONFIG_CLUBINS_CAP_TRANSPORT_HTTP
#include "transport_http.h"
#endif
#if CONFIG_CLUBINS_CAP_TRANSPORT_MQTT
#include "transport_mqtt.h"
#endif
#if CONFIG_CLUBINS_CAP_OTA
#include "ops_ota.h"
#endif
#if CONFIG_CLUBINS_CAP_PROVISIONING
#include "ops_provisioning.h"
#endif
#if CONFIG_CLUBINS_CAP_DEEP_SLEEP
#include "ops_sleep.h"
#endif
#if CONFIG_CLUBINS_CAP_LOGGING_DEFMT
#include "logging_defmt.h"
#endif
#if CONFIG_CLUBINS_CAP_OPS_STATUS
#include "ops_status.h"
#endif

/** One composed transport/ops hook pair (sensors live in the registry). */
typedef struct {
    void (*init)(void);
    void (*tick)(void);
} clubins_hook_t;

/** The composed transport/ops hooks, in stable registry order. The
 * trailing sentinel keeps the table non-empty when nothing is composed
 * (the all-off build must compile); it is never counted. */
static const clubins_hook_t s_hooks[] = {
#if CONFIG_CLUBINS_CAP_TRANSPORT_HTTP
    {clubins_transport_http_init, clubins_transport_http_tick},
#endif
#if CONFIG_CLUBINS_CAP_TRANSPORT_MQTT
    {clubins_transport_mqtt_init, clubins_transport_mqtt_tick},
#endif
#if CONFIG_CLUBINS_CAP_TRANSPORT_MTLS
    {clubins_transport_mtls_init, clubins_transport_mtls_tick},
#endif
#if CONFIG_CLUBINS_CAP_OTA
    {clubins_ops_ota_init, clubins_ops_ota_tick},
#endif
#if CONFIG_CLUBINS_CAP_PROVISIONING
    {clubins_ops_provisioning_init, clubins_ops_provisioning_tick},
#endif
#if CONFIG_CLUBINS_CAP_DEEP_SLEEP
    {clubins_ops_sleep_init, clubins_ops_sleep_tick},
#endif
#if CONFIG_CLUBINS_CAP_LOGGING_DEFMT
    {clubins_logging_defmt_init, clubins_logging_defmt_tick},
#endif
#if CONFIG_CLUBINS_CAP_OPS_STATUS
    {clubins_ops_status_init, clubins_ops_status_tick},
#endif
    {NULL, NULL}, /* sentinel: never counted */
};

static size_t hook_count(void)
{
    return (sizeof(s_hooks) / sizeof(s_hooks[0])) - 1;
}

/* Driver instances — the composition owns them (V2 boxes them; the C port
 * keeps static storage, which is all the scaffold drivers need and keeps
 * the composition allocation-free). */
#if CONFIG_CLUBINS_CAP_SENSOR_DECIBEL
static clubins_decibel_driver_t s_decibel;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PAX
static clubins_pir_driver_t s_pir;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BME280
static clubins_bme280_driver_t s_bme280;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SHT4X
static clubins_sht4x_driver_t s_sht4x;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SCD4X
static clubins_scd4x_driver_t s_scd4x;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PMS5003
static clubins_pms5003_driver_t s_pms5003;
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BH1750
static clubins_bh1750_driver_t s_bh1750;
#endif

static clubins_registry_t s_registry;
static uint32_t s_samples;
static bool s_composed;

static void s_on_measurement(void *sink_ctx, const clubins_measurement_t *measurement)
{
    (void)sink_ctx;
    (void)measurement;
    /* The publish path (encode + transport) lands with the transport
     * tasks; the scaffold counts what the drivers deliver. */
    s_samples++;
}

/* True when any sensor-driver component is composed; the registration
 * helper below exists only then (an all-off build would otherwise carry
 * dead code, and -Werror carries it to the build). */
#if CONFIG_CLUBINS_CAP_SENSOR_DECIBEL || CONFIG_CLUBINS_CAP_SENSOR_PAX ||                          \
    CONFIG_CLUBINS_CAP_SENSOR_BME280 || CONFIG_CLUBINS_CAP_SENSOR_SHT4X ||                         \
    CONFIG_CLUBINS_CAP_SENSOR_SCD4X || CONFIG_CLUBINS_CAP_SENSOR_PMS5003 ||                        \
    CONFIG_CLUBINS_CAP_SENSOR_BH1750
#define CLUBINS_HAS_SENSOR_COMPONENTS 1
#else
#define CLUBINS_HAS_SENSOR_COMPONENTS 0
#endif

/** Register one driver, aborting when the registry is full — V2's
 * `.expect("registry sizing")`: a driver dropped here would silently lose
 * a measurement stream. */
#if CLUBINS_HAS_SENSOR_COMPONENTS
static void s_push(const clubins_sensor_ops_t *ops, void *ctx)
{
    if (clubins_registry_push(&s_registry, ops, ctx) != CLUBINS_REGISTRY_OK) {
        abort();
    }
}
#endif

void clubins_app_compose(void)
{
    if (s_composed) {
        return;
    }

    clubins_registry_init(&s_registry);

#if CONFIG_CLUBINS_CAP_SENSOR_DECIBEL
    clubins_decibel_driver_new(&s_decibel);
    s_push(&clubins_decibel_sensor_ops, &s_decibel);
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PAX
    clubins_pir_driver_new(&s_pir);
    s_push(&clubins_pir_sensor_ops, &s_pir);
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BME280
    clubins_bme280_driver_new(&s_bme280);
    s_push(&clubins_bme280_sensor_ops, &s_bme280);
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SHT4X
    clubins_sht4x_driver_new(&s_sht4x);
    s_push(&clubins_sht4x_sensor_ops, &s_sht4x);
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_SCD4X
    clubins_scd4x_driver_new(&s_scd4x);
    s_push(&clubins_scd4x_sensor_ops, &s_scd4x);
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_PMS5003
    clubins_pms5003_driver_new(&s_pms5003);
    s_push(&clubins_pms5003_sensor_ops, &s_pms5003);
#endif
#if CONFIG_CLUBINS_CAP_SENSOR_BH1750
    clubins_bh1750_driver_new(&s_bh1750);
    s_push(&clubins_bh1750_sensor_ops, &s_bh1750);
#endif

    s_composed = true;
}

clubins_registry_t *clubins_app_registry(void)
{
    return s_composed ? &s_registry : NULL;
}

size_t clubins_app_composed_sensors(void)
{
    return clubins_registry_len(&s_registry);
}

size_t clubins_app_composed_hooks(void)
{
    return hook_count();
}

uint32_t clubins_app_sampled_measurements(void)
{
    return s_samples;
}

void clubins_app_init_all(void)
{
    clubins_registry_init_all(&s_registry);
    for (size_t i = 0; i < hook_count(); i++) {
        s_hooks[i].init();
    }
}

void clubins_app_tick(void)
{
    clubins_registry_sample_all(&s_registry, s_on_measurement, NULL);
    for (size_t i = 0; i < hook_count(); i++) {
        s_hooks[i].tick();
    }
}
