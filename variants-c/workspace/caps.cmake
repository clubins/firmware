# The flag → component mapping — the single source of truth shared by the
# device composition (main/CMakeLists.txt) and the host harness
# (host/CMakeLists.txt). The C port of V2's firmware/Cargo.toml [features]
# table: every public capability leaf maps 1:1 to a workspace component,
# exactly as V2 maps each leaf to an optional crate dependency.
#
# CONFIG_CLUBINS_CAP_* variables come from sdkconfig on device (ESP-IDF
# exports every Kconfig symbol as a CMake variable) and from -D definitions
# on the host (tools/flag-matrix.sh mirrors the Kconfig select graph).
#
# The three lists below are parallel: index i across CLUBINS_CAP_FLAGS,
# CLUBINS_CAP_COMPONENTS, and CLUBINS_CAP_SOURCES describes one capability
# (flat lists — CMake flattens quoted "a;b;c" tuples in set()).
#
# mTLS is an MQTT connection mode (V2 parity: transport-mtls selects
# transport-mqtt and has no crate of its own): it has no component entry
# here and gates content inside transport_mqtt via caps_config.h.
#
# The plumbing flags (DRIVER_I2C/UART/GPIO, NET_WIFI, PROTO_V2) stay
# Kconfig-internal dependency carriers, mirroring V1/V2: they select bus
# and stack components when those tasks land. On the host they are defined
# but consume no sources yet — listed below so the flag matrix's -D
# definitions don't warn as unused CLI variables in the meantime.
#
# Included at top level by both consumers; they call the functions below.

set(CLUBINS_APP_DIR "${CMAKE_CURRENT_LIST_DIR}")

set(CLUBINS_PLUMBING_FLAG_VALUES
    "${CONFIG_CLUBINS_CAP_DRIVER_I2C}"
    "${CONFIG_CLUBINS_CAP_DRIVER_UART}"
    "${CONFIG_CLUBINS_CAP_DRIVER_GPIO}"
    "${CONFIG_CLUBINS_CAP_NET_WIFI}"
    "${CONFIG_CLUBINS_CAP_PROTO_V2}"
)

# TRANSPORT_MTLS is a public leaf but a mode of the MQTT component, not a
# component of its own: no CMake conditional consumes it (content inside
# transport_mqtt reads it via caps_config.h), so touch it here to keep the
# matrix's -D definitions warning-free.
set(CLUBINS_MODE_FLAG_VALUES "${CONFIG_CLUBINS_CAP_TRANSPORT_MTLS}")

set(CLUBINS_CAP_FLAGS
    SENSOR_DECIBEL
    SENSOR_PAX
    SENSOR_BME280
    SENSOR_SHT4X
    SENSOR_SCD4X
    SENSOR_PMS5003
    SENSOR_BH1750
    TRANSPORT_HTTP
    TRANSPORT_MQTT
    OTA
    PROVISIONING
    DEEP_SLEEP
    LOGGING_DEFMT
    OPS_STATUS
)

set(CLUBINS_CAP_COMPONENTS
    drv_decibel
    drv_pax
    drv_bme280
    drv_sht4x
    drv_scd4x
    drv_pms5003
    drv_bh1750
    transport_http
    transport_mqtt
    ops_ota
    ops_provisioning
    ops_sleep
    logging_defmt
    ops_status
)

# Sources are project-root relative; each component owns exactly one
# translation unit in this scaffold.
set(CLUBINS_CAP_SOURCES
    "components/drivers/drv_decibel/src/decibel_driver.c"
    "components/drivers/drv_pax/src/pir_driver.c"
    "components/drivers/drv_bme280/src/bme280_driver.c"
    "components/drivers/drv_sht4x/src/sht4x_driver.c"
    "components/drivers/drv_scd4x/src/scd4x_driver.c"
    "components/drivers/drv_pms5003/src/pms5003_driver.c"
    "components/drivers/drv_bh1750/src/bh1750_driver.c"
    "components/transports/transport_http/src/transport_http.c"
    "components/transports/transport_mqtt/src/transport_mqtt.c"
    "components/ops/ops_ota/src/ops_ota.c"
    "components/ops/ops_provisioning/src/ops_provisioning.c"
    "components/ops/ops_sleep/src/ops_sleep.c"
    "components/ops/logging_defmt/src/logging_defmt.c"
    "components/ops/ops_status/src/ops_status.c"
)

# The component names the composed app requires, in stable registry order —
# the consumer in main/CMakeLists.txt passes the result as REQUIRES.
function(clubins_caps_required_components out_var)
    set(requires)
    list(LENGTH CLUBINS_CAP_FLAGS count)
    math(EXPR last "${count} - 1")
    foreach(i RANGE "${last}")
        list(GET CLUBINS_CAP_FLAGS "${i}" flag)
        list(GET CLUBINS_CAP_COMPONENTS "${i}" component)
        if(CONFIG_CLUBINS_CAP_${flag})
            list(APPEND requires "${component}")
        endif()
    endforeach()
    set(${out_var} "${requires}" PARENT_SCOPE)
endfunction()

# The host mirror of the same composition: sources and include dirs of the
# components this combination enables. A flag wired to a composition arm
# whose source is missing here is a link error in the combination that
# broke it — the property the flag matrix verifies.
function(clubins_caps_host_sources out_sources out_include_dirs)
    set(sources)
    set(dirs)
    list(LENGTH CLUBINS_CAP_FLAGS count)
    math(EXPR last "${count} - 1")
    foreach(i RANGE "${last}")
        list(GET CLUBINS_CAP_FLAGS "${i}" flag)
        list(GET CLUBINS_CAP_SOURCES "${i}" source)
        if(CONFIG_CLUBINS_CAP_${flag})
            list(APPEND sources "${CLUBINS_APP_DIR}/${source}")
            get_filename_component(src_dir "${CLUBINS_APP_DIR}/${source}" DIRECTORY)
            get_filename_component(inc_dir "${src_dir}/../include" ABSOLUTE)
            list(APPEND dirs "${inc_dir}")
        endif()
    endforeach()
    set(${out_sources} "${sources}" PARENT_SCOPE)
    set(${out_include_dirs} "${dirs}" PARENT_SCOPE)
endfunction()
