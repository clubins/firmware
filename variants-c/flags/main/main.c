//! Device entry point for the ESP32-S3 — the C port of V1's thin main.
//!
//! The main loop is FreeRTOS-timer-driven: a periodic timer ticks the
//! capability registry once per period, mirroring V1's superloop (sample →
//! window → encode → publish) at scaffold fidelity. Real bring-up (Wi-Fi,
//! NVS config, drivers) lands with the downstream C tasks; this entry only
//! proves the Xtensa personality builds, links, and drives the registry.

#include <stdbool.h>
#include <stdint.h>

#include "cap_registry.h"
#include "caps_config.h"

#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/timers.h"

static const char *TAG = "clubins-flags";

/** Loop period. The runtime cadence (report_interval_s in the committed
 * config) lands with the configuration task; the scaffold fixes 1 s. */
#define CLUBINS_LOOP_PERIOD_MS 1000

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

    clubins_registry_init_all();

    TimerHandle_t loop_timer = xTimerCreate("clubins-loop", pdMS_TO_TICKS(CLUBINS_LOOP_PERIOD_MS),
                                            pdTRUE, NULL, loop_tick);
    configASSERT(loop_timer != NULL);
    configASSERT(xTimerStart(loop_timer, 0) == pdPASS);
}
