//! NVS override adapter — the device binding of config.h's override-source
//! interface (the NVS tier of the precedence rule).
//!
//! Reads the "cfg" NVS namespace, initialized lazily on first use (boot
//! config load is single-threaded). The loader's dotted key space maps
//! onto NVS's 15-char key space:
//!
//!     sensor.id                     -> sensor_id
//!     sampling.decibel_window_ms    -> db_window_ms
//!     sampling.report_interval_s    -> report_s
//!     sampling.offline_buffer       -> offline_buf
//!     sensors.bme280.i2c_address    -> bme280_addr
//!     sensors.pir.gpio_pin          -> pir_pin
//!     transport.active              -> transport
//!     transport.http_endpoint       -> http_url
//!     transport.mqtt.broker         -> mqtt_broker
//!     transport.mqtt.port           -> mqtt_port
//!     decibel.overload_threshold_db -> db_overload
//!     decibel.calibration_offset_db -> db_cal
//!
//! Types on NVS: i32 fields as int32, doubles as 8-byte blobs, strings as
//! strings. A too-long string is a source error, never a truncation.
//! Secrets (API key, Wi-Fi credentials, client certificate/key) are also
//! NVS residents — but they are read by the transport/provisioning tasks
//! that consume them, never through this interface and never into the
//! config pool. Device-only file: it carries IDF headers and is compiled
//! only in the ESP-IDF component build.

#pragma once

#include "config.h"

#ifdef __cplusplus
extern "C" {
#endif

/** The NVS-backed override source for clubins_config_load(). Never NULL;
 * when NVS is unavailable every getter reports the key as absent, so the
 * pool falls back to the file tier and built-ins. The returned object has
 * static storage — do not free. */
const clubins_config_overrides_t *clubins_config_nvs_overrides(void);

#ifdef __cplusplus
}
#endif
