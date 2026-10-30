//! Capability-flag indirection — the C stand-in for cargo's feature cfg.
//!
//! On ESP-IDF, the generated sdkconfig.h defines CONFIG_CLUBINS_CAP_* (1)
//! for every enabled Kconfig capability. On the host, the CTest harness
//! passes -DCONFIG_CLUBINS_CAP_*=1 for the enabled ones (tools/
//! flag-matrix.sh mirrors the Kconfig select graph). Anything unset reads
//! as 0, so "off" is the absence of a define — the same semantics V1's
//! cargo features had. Core code never includes sdkconfig.h directly; it
//! includes this header, which is why the same sources build on both
//! personalities.

#pragma once

#if defined(__has_include)
#if __has_include(<sdkconfig.h>)
#include <sdkconfig.h>
#endif
#endif

/* Public capability leaves (spec §Flag Matrix) */
#ifndef CONFIG_CLUBINS_CAP_SENSOR_DECIBEL
#define CONFIG_CLUBINS_CAP_SENSOR_DECIBEL 0
#endif
#ifndef CONFIG_CLUBINS_CAP_SENSOR_PAX
#define CONFIG_CLUBINS_CAP_SENSOR_PAX 0
#endif
#ifndef CONFIG_CLUBINS_CAP_SENSOR_BME280
#define CONFIG_CLUBINS_CAP_SENSOR_BME280 0
#endif
#ifndef CONFIG_CLUBINS_CAP_SENSOR_SHT4X
#define CONFIG_CLUBINS_CAP_SENSOR_SHT4X 0
#endif
#ifndef CONFIG_CLUBINS_CAP_SENSOR_SCD4X
#define CONFIG_CLUBINS_CAP_SENSOR_SCD4X 0
#endif
#ifndef CONFIG_CLUBINS_CAP_SENSOR_PMS5003
#define CONFIG_CLUBINS_CAP_SENSOR_PMS5003 0
#endif
#ifndef CONFIG_CLUBINS_CAP_SENSOR_BH1750
#define CONFIG_CLUBINS_CAP_SENSOR_BH1750 0
#endif
#ifndef CONFIG_CLUBINS_CAP_TRANSPORT_HTTP
#define CONFIG_CLUBINS_CAP_TRANSPORT_HTTP 0
#endif
#ifndef CONFIG_CLUBINS_CAP_TRANSPORT_MQTT
#define CONFIG_CLUBINS_CAP_TRANSPORT_MQTT 0
#endif
#ifndef CONFIG_CLUBINS_CAP_TRANSPORT_MTLS
#define CONFIG_CLUBINS_CAP_TRANSPORT_MTLS 0
#endif
#ifndef CONFIG_CLUBINS_CAP_OTA
#define CONFIG_CLUBINS_CAP_OTA 0
#endif
#ifndef CONFIG_CLUBINS_CAP_PROVISIONING
#define CONFIG_CLUBINS_CAP_PROVISIONING 0
#endif
#ifndef CONFIG_CLUBINS_CAP_DEEP_SLEEP
#define CONFIG_CLUBINS_CAP_DEEP_SLEEP 0
#endif
#ifndef CONFIG_CLUBINS_CAP_LOGGING_DEFMT
#define CONFIG_CLUBINS_CAP_LOGGING_DEFMT 0
#endif
#ifndef CONFIG_CLUBINS_CAP_OPS_STATUS
#define CONFIG_CLUBINS_CAP_OPS_STATUS 0
#endif

/* Internal plumbing flags — selected by the leaves in Kconfig.projbuild;
 * on the host the flag-matrix script defines them to mirror that graph. */
#ifndef CONFIG_CLUBINS_CAP_DRIVER_I2C
#define CONFIG_CLUBINS_CAP_DRIVER_I2C 0
#endif
#ifndef CONFIG_CLUBINS_CAP_DRIVER_UART
#define CONFIG_CLUBINS_CAP_DRIVER_UART 0
#endif
#ifndef CONFIG_CLUBINS_CAP_DRIVER_GPIO
#define CONFIG_CLUBINS_CAP_DRIVER_GPIO 0
#endif
#ifndef CONFIG_CLUBINS_CAP_NET_WIFI
#define CONFIG_CLUBINS_CAP_NET_WIFI 0
#endif
#ifndef CONFIG_CLUBINS_CAP_PROTO_V2
#define CONFIG_CLUBINS_CAP_PROTO_V2 0
#endif
