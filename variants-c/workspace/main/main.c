//! Device entry point for the ESP32-S3 — the C port of V2's slim
//! composition root (firmware/src/main.rs).
//!
//! Config in, composed sensors + hooks out, sampling loop runs: this file
//! is device glue only — boot, the composition log line, and the periodic
//! FreeRTOS timer that drives clubins_app_tick. The config pool (V2's
//! Config) and the real publish path land with the workspace's config and
//! transport tasks; until then the scaffold composes every compiled-in
//! capability and samples at the loop period.

#include <inttypes.h>

#include "app_compose.h"

#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/timers.h"
#include "sdkconfig.h"

static const char *TAG = "clubins-workspace";

/** Loop period from the main component's Kconfig (main/Kconfig). */
#define CLUBINS_LOOP_PERIOD_MS CONFIG_CLUBINS_LOOP_PERIOD_MS

static void loop_tick(TimerHandle_t timer)
{
    (void)timer;
    clubins_app_tick();
}

void app_main(void)
{
    clubins_app_compose();

    ESP_LOGI(TAG, "clubins workspace firmware: %u sensor(s) + %u hook(s) composed",
             (unsigned)clubins_app_composed_sensors(), (unsigned)clubins_app_composed_hooks());

    clubins_app_init_all();

    TimerHandle_t loop_timer = xTimerCreate("clubins-loop", pdMS_TO_TICKS(CLUBINS_LOOP_PERIOD_MS),
                                            pdTRUE, NULL, loop_tick);
    configASSERT(loop_timer != NULL);
    configASSERT(xTimerStart(loop_timer, 0) == pdPASS);
}
