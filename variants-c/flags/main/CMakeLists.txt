# The main component: the device personality of the flags variant.
#
# main.c, cap_registry.c, and every enabled stub compile here. caps.cmake is
# included for the CONFIG_CLUBINS_CAP_* -> source mapping so the conditional
# compilation is shared verbatim with the host harness. Stub sources carry no
# IDF headers, so the same files also build on the host (host/CMakeLists.txt).
# config.c is the runtime-config loader (tomlc99 is vendored under
# third_party/); the NVS override adapter joins it in the next commit.

include(${CMAKE_CURRENT_LIST_DIR}/../caps.cmake)

set(CLUBINS_SRCS
    "main.c"
    "cap_registry.c"
    "measurement.c"
    "http_json.c"
    "config.c"
    "${CMAKE_CURRENT_LIST_DIR}/../third_party/tomlc99/toml.c"
    ${CLUBINS_CAP_SOURCES}
)

idf_component_register(
    SRCS ${CLUBINS_SRCS}
    INCLUDE_DIRS "."
    PRIV_INCLUDE_DIRS "${CMAKE_CURRENT_LIST_DIR}/../third_party/tomlc99"
    PRIV_REQUIRES freertos log
)
