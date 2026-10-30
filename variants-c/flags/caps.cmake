# Capability -> stub-source mapping — the single source of truth shared by
# the device component build (main/CMakeLists.txt) and the host CTest
# harness (host/CMakeLists.txt). Both personalities include this file, so
# the CMake conditionals gating stub compilation are literally the same
# code on both sides.
#
# CONFIG_CLUBINS_CAP_* variables come from sdkconfig on device (ESP-IDF
# exports every Kconfig symbol as a CMake variable) and from -D definitions
# on the host (tools/flag-matrix.sh mirrors the Kconfig select graph). A
# disabled capability's stub file is not compiled at all — the CMake
# conditional is the "compiles to nothing when off" half of the framework.
#
# Included at top level by both callers; the list they consume is
# CLUBINS_CAP_SOURCES.

set(CLUBINS_APP_DIR "${CMAKE_CURRENT_LIST_DIR}")

set(CLUBINS_CAP_SOURCES)

# Plumbing flags (DRIVER_I2C/UART/GPIO, NET_WIFI, PROTO_V2) are consumed in
# C via caps_config.h — and once the driver/transport tasks land, in CMake
# conditionals of their own. Touch them here so the host matrix's -D
# definitions don't warn as unused CLI variables in the meantime.
set(CLUBINS_PLUMBING_FLAG_VALUES
    "${CONFIG_CLUBINS_CAP_DRIVER_I2C}"
    "${CONFIG_CLUBINS_CAP_DRIVER_UART}"
    "${CONFIG_CLUBINS_CAP_DRIVER_GPIO}"
    "${CONFIG_CLUBINS_CAP_NET_WIFI}"
    "${CONFIG_CLUBINS_CAP_PROTO_V2}"
)

if(CONFIG_CLUBINS_CAP_SENSOR_DECIBEL)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/sensors/pdm_mic.c")
endif()

if(CONFIG_CLUBINS_CAP_SENSOR_PAX)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/sensors/pir.c")
endif()

if(CONFIG_CLUBINS_CAP_SENSOR_BME280)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/sensors/bme280.c")
endif()

if(CONFIG_CLUBINS_CAP_SENSOR_SHT4X)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/sensors/sht4x.c")
endif()

if(CONFIG_CLUBINS_CAP_SENSOR_SCD4X)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/sensors/scd4x.c")
endif()

if(CONFIG_CLUBINS_CAP_SENSOR_PMS5003)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/sensors/pms5003.c")
endif()

if(CONFIG_CLUBINS_CAP_SENSOR_BH1750)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/sensors/bh1750.c")
endif()

if(CONFIG_CLUBINS_CAP_TRANSPORT_HTTP)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/transports/http.c")
endif()

if(CONFIG_CLUBINS_CAP_TRANSPORT_MQTT)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/transports/mqtt.c")
endif()

if(CONFIG_CLUBINS_CAP_TRANSPORT_MTLS)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/ops/mtls.c")
endif()

if(CONFIG_CLUBINS_CAP_OTA)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/ops/ota.c")
endif()

if(CONFIG_CLUBINS_CAP_PROVISIONING)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/ops/ble_provisioning.c")
endif()

if(CONFIG_CLUBINS_CAP_DEEP_SLEEP)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/ops/deep_sleep.c")
endif()

if(CONFIG_CLUBINS_CAP_LOGGING_DEFMT)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/ops/logging_defmt.c")
endif()

if(CONFIG_CLUBINS_CAP_OPS_STATUS)
    list(APPEND CLUBINS_CAP_SOURCES "${CLUBINS_APP_DIR}/main/stubs/ops/status.c")
endif()
