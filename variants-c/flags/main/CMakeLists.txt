# The main component: the device personality of the flags variant.
#
# main.c, cap_registry.c, and every enabled stub compile here. caps.cmake is
# included for the CONFIG_CLUBINS_CAP_* -> source mapping so the conditional
# compilation is shared verbatim with the host harness. Stub sources carry no
# IDF headers, so the same files also build on the host (host/CMakeLists.txt).
# config.c is the runtime-config loader (tomlc99 is vendored under
# third_party/); nvs_overrides.c binds the NVS tier of the precedence rule
# (device-only — it carries IDF headers). The canonical config file is
# embedded into the binary as the file tier's bytes.

include(${CMAKE_CURRENT_LIST_DIR}/../caps.cmake)

set(CLUBINS_SRCS
    "main.c"
    "cap_registry.c"
    "measurement.c"
    "http_json.c"
    "config.c"
    "nvs_overrides.c"
    ${CLUBINS_CAP_SOURCES}
)

idf_component_register(
    SRCS ${CLUBINS_SRCS}
    INCLUDE_DIRS "."
    PRIV_INCLUDE_DIRS "${CMAKE_CURRENT_LIST_DIR}/../third_party/tomlc99"
    PRIV_REQUIRES freertos log nvs_flash
)

# The committed config file, embedded as the file tier's bytes (symbols:
# _binary_default_toml_start / _binary_default_toml_end; RENAME keeps them
# readable). Single source of truth stays at config/default.toml.
target_add_binary_data(${COMPONENT_LIB} "${CMAKE_CURRENT_LIST_DIR}/../config/default.toml" TEXT
    RENAME "default_toml")

# Vendored tomlc99 builds in its own bucket, outside the component's warning
# set: IDF compiles components with -Werror=all, which turns upstream's
# char-subscripts sites in toml.c into hard errors. Vendored sources stay at
# their pinned upstream revision (no patches), so warnings are suppressed for
# this target only — the same third-party-owns-its-warnings policy as the
# host harness.
add_library(tomlc99 STATIC
    "${CMAKE_CURRENT_LIST_DIR}/../third_party/tomlc99/toml.c")
target_include_directories(tomlc99 SYSTEM PUBLIC
    "${CMAKE_CURRENT_LIST_DIR}/../third_party/tomlc99")
target_compile_options(tomlc99 PRIVATE -w)
target_link_libraries(${COMPONENT_LIB} PRIVATE tomlc99)
