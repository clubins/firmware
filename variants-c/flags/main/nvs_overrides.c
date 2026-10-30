//! NVS override adapter implementation (device only — see nvs_overrides.h).

#include "nvs_overrides.h"

#include <stdint.h>
#include <string.h>

#include "esp_log.h"
#include "nvs.h"
#include "nvs_flash.h"

static const char *TAG = "clubins-cfg";

#define NVS_NAMESPACE "cfg"
#define NVS_DOUBLE_BLOB_LEN 8

typedef struct {
    const char *dotted;
    const char *nvs_key;
} key_map_t;

/** The loader's dotted key space -> NVS key space (all NVS keys respect
 * the 15-char cap). Unmapped keys read as absent. */
static const key_map_t KEY_MAP[] = {
    {"sensor.id", "sensor_id"},
    {"sampling.decibel_window_ms", "db_window_ms"},
    {"sampling.report_interval_s", "report_s"},
    {"sampling.offline_buffer", "offline_buf"},
    {"sensors.bme280.i2c_address", "bme280_addr"},
    {"sensors.pir.gpio_pin", "pir_pin"},
    {"transport.active", "transport"},
    {"transport.http_endpoint", "http_url"},
    {"transport.mqtt.broker", "mqtt_broker"},
    {"transport.mqtt.port", "mqtt_port"},
    {"decibel.overload_threshold_db", "db_overload"},
    {"decibel.calibration_offset_db", "db_cal"},
};

static const char *map_key(const char *dotted)
{
    for (size_t i = 0; i < sizeof(KEY_MAP) / sizeof(KEY_MAP[0]); i++) {
        if (strcmp(dotted, KEY_MAP[i].dotted) == 0) {
            return KEY_MAP[i].nvs_key;
        }
    }
    return NULL;
}

/** Open the config namespace once. 0 means "NVS unusable" — a fresh flash
 * without the namespace is the normal case and degrades to absent. */
static nvs_handle_t ensure_open(void)
{
    static nvs_handle_t handle = 0;
    static bool attempted = false;
    if (attempted) {
        return handle;
    }
    attempted = true;

    esp_err_t err = nvs_flash_init();
    if (err == ESP_ERR_NVS_NO_FREE_PAGES || err == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        err = nvs_flash_init();
    }
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "nvs_flash_init failed: %s", esp_err_to_name(err));
        return 0;
    }
    if (nvs_open(NVS_NAMESPACE, NVS_READONLY, &handle) != ESP_OK) {
        ESP_LOGW(TAG, "nvs_open(%s) found nothing — overrides tier empty", NVS_NAMESPACE);
        handle = 0;
    }
    return handle;
}

static int get_str(void *ctx, const char *key, char *out, size_t cap)
{
    (void)ctx;
    nvs_handle_t handle = ensure_open();
    const char *nvs_key = map_key(key);
    if (handle == 0 || nvs_key == NULL) {
        return 0;
    }
    size_t len = cap;
    esp_err_t err = nvs_get_str(handle, nvs_key, out, &len);
    if (err == ESP_OK) {
        return 1;
    }
    if (err == ESP_ERR_NVS_NOT_FOUND) {
        return 0;
    }
    ESP_LOGW(TAG, "nvs_get_str(%s): %s", nvs_key, esp_err_to_name(err));
    return -1; /* includes INVALID_LENGTH — reject, never truncate */
}

static int get_i32(void *ctx, const char *key, int32_t *out)
{
    (void)ctx;
    nvs_handle_t handle = ensure_open();
    const char *nvs_key = map_key(key);
    if (handle == 0 || nvs_key == NULL) {
        return 0;
    }
    esp_err_t err = nvs_get_i32(handle, nvs_key, out);
    if (err == ESP_OK) {
        return 1;
    }
    if (err == ESP_ERR_NVS_NOT_FOUND) {
        return 0;
    }
    ESP_LOGW(TAG, "nvs_get_i32(%s): %s", nvs_key, esp_err_to_name(err));
    return -1;
}

static int get_f64(void *ctx, const char *key, double *out)
{
    (void)ctx;
    nvs_handle_t handle = ensure_open();
    const char *nvs_key = map_key(key);
    if (handle == 0 || nvs_key == NULL) {
        return 0;
    }
    size_t len = NVS_DOUBLE_BLOB_LEN;
    double value = 0.0;
    esp_err_t err = nvs_get_blob(handle, nvs_key, &value, &len);
    if (err == ESP_OK && len == NVS_DOUBLE_BLOB_LEN) {
        *out = value;
        return 1;
    }
    if (err == ESP_ERR_NVS_NOT_FOUND) {
        return 0;
    }
    ESP_LOGW(TAG, "nvs_get_blob(%s): %s (len=%u)", nvs_key, esp_err_to_name(err),
             (unsigned)len);
    return -1;
}

const clubins_config_overrides_t *clubins_config_nvs_overrides(void)
{
    static const clubins_config_overrides_t source = {
        .ctx = NULL,
        .get_str = get_str,
        .get_i32 = get_i32,
        .get_f64 = get_f64,
    };
    return &source;
}
