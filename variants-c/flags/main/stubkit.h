//! Stub lifecycle declarations — one gated pair per capability leaf.
//!
//! Every stub is two empty functions (real implementations ship with the
//! downstream driver/transport/ops tasks, docs/plan/build-dag.md). When a
//! capability is disabled its stub source is not compiled at all (see
//! caps.cmake), and its declarations here are gated by the same flag, so a
//! dangling reference is impossible.

#pragma once

#include "caps_config.h"

#if CONFIG_CLUBINS_CAP_SENSOR_DECIBEL
void clubins_stub_pdm_mic_init(void);
void clubins_stub_pdm_mic_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_SENSOR_PAX
void clubins_stub_pir_init(void);
void clubins_stub_pir_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_SENSOR_BME280
void clubins_stub_bme280_init(void);
void clubins_stub_bme280_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_SENSOR_SHT4X
void clubins_stub_sht4x_init(void);
void clubins_stub_sht4x_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_SENSOR_SCD4X
void clubins_stub_scd4x_init(void);
void clubins_stub_scd4x_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_SENSOR_PMS5003
void clubins_stub_pms5003_init(void);
void clubins_stub_pms5003_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_SENSOR_BH1750
void clubins_stub_bh1750_init(void);
void clubins_stub_bh1750_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_TRANSPORT_HTTP
void clubins_stub_http_init(void);
void clubins_stub_http_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_TRANSPORT_MQTT
void clubins_stub_mqtt_init(void);
void clubins_stub_mqtt_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_TRANSPORT_MTLS
void clubins_stub_mtls_init(void);
void clubins_stub_mtls_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_OTA
void clubins_stub_ota_init(void);
void clubins_stub_ota_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_PROVISIONING
void clubins_stub_ble_provisioning_init(void);
void clubins_stub_ble_provisioning_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_DEEP_SLEEP
void clubins_stub_deep_sleep_init(void);
void clubins_stub_deep_sleep_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_LOGGING_DEFMT
void clubins_stub_logging_defmt_init(void);
void clubins_stub_logging_defmt_tick(void);
#endif

#if CONFIG_CLUBINS_CAP_OPS_STATUS
void clubins_stub_status_init(void);
void clubins_stub_status_tick(void);
#endif
