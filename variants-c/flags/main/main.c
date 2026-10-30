//! Device entry point for the ESP32-S3 — the C port of V1's thin main.
//!
//! Boot loads the static config pool (built-ins < embedded default.toml <
//! NVS overrides), then brings up the capability registry and hands to the
//! loop: a periodic FreeRTOS timer ticks the registry once per period,
//! mirroring V1's superloop (sample → window → encode → publish) at
//! scaffold fidelity. Real bring-up (Wi-Fi, drivers, transports) lands
//! with the downstream C tasks, which read the pool via config.h.

#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>

#include "cap_registry.h"
#include "caps_config.h"
#include "config.h"
#include "nvs_overrides.h"

#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/timers.h"

static const char *TAG = "clubins-flags";

/** The committed config file, embedded at build time (main/CMakeLists.txt
 * target_add_binary_data ... RENAME default_toml). The device copy of the
 * file tier — config/default.toml stays the single committed source. */
extern const uint8_t clubins_default_toml_start[] asm("_binary_default_toml_start");
extern const uint8_t clubins_default_toml_end[] asm("_binary_default_toml_end");

/** Loop period. The sampling cadence stays fixed; the per-type reporting
 * cadence (sampling.report_interval_s in the pool) is consumed by the
 * transport tasks that publish measurements. */
#define CLUBINS_LOOP_PERIOD_MS 1000

static const char *transport_name(clubins_transport_active_t active)
{
    switch (active) {
    case CLUBINS_TRANSPORT_MQTT:
        return "mqtt";
    case CLUBINS_TRANSPORT_HTTP:
        return "http";
    default:
        return "?";
    }
}

static void log_config_summary(void)
{
    const clubins_config_t *cfg = clubins_config_pool();
    if (cfg == NULL) {
        ESP_LOGE(TAG, "config pool missing after load");
        return;
    }
    ESP_LOGI(TAG, "config: sensor=%s transport=%s report_every=%" PRIi32 "s", cfg->sensor_id,
             transport_name(cfg->transport_active), cfg->report_interval_s);
}

static void load_config(void)
{
    clubins_config_warn_t warn;
    const clubins_config_status_t status = clubins_config_load(
        (const char *)clubins_default_toml_start, clubins_config_nvs_overrides(), &warn);
    if (status != CLUBINS_CONFIG_OK) {
        /* Tolerance contract: the pool is still usable (built-ins + NVS). */
        ESP_LOGE(TAG, "config file tier failed (%d) — built-ins + NVS apply", (int)status);
    }
    for (size_t i = 0; i < warn.count; i++) {
        ESP_LOGW(TAG, "config %s: %s", warn.entries[i].key,
                 clubins_config_warn_kind_name(warn.entries[i].kind));
    }
    log_config_summary();
}

static void loop_tick(TimerHandle_t timer)
{
    (void)timer;
    clubins_registry_tick_all();
}

void app_main(void)
{
    const clubins_module_t *modules[CLUBINS_CAP_MAX_MODULES];
    const size_t module_count = clubins_registry_modules(modules, CLUBINS_CAP_MAX_MODULES);

    ESP_LOGI(TAG, "clubins flags firmware: %u capability module(s) compiled in",
             (unsigned)module_count);

    load_config();
    clubins_registry_init_all();

    TimerHandle_t loop_timer = xTimerCreate("clubins-loop", pdMS_TO_TICKS(CLUBINS_LOOP_PERIOD_MS),
                                            pdTRUE, NULL, loop_tick);
    configASSERT(loop_timer != NULL);
    configASSERT(xTimerStart(loop_timer, 0) == pdPASS);
}
