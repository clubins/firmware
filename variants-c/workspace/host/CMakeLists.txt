# Host test harness for the C2 workspace variant — the platform-agnostic
# sources of the device project recompiled for the host machine: CTest +
# Unity, no ESP-IDF anywhere. The flag-matrix script (tools/flag-matrix.sh)
# drives this file 17 times (all-off, each public leaf alone with its
# Kconfig selects, all-on), passing the combination as
# -DCONFIG_CLUBINS_CAP_*=1 definitions; caps.cmake maps those to the same
# component sources the device composition uses.
#
# Same sources, two personalities: components/clubins_core and
# main/app_compose.c compile here byte-identical to the device build —
# main/main.c (the only IDF-headered file) is deliberately excluded.
cmake_minimum_required(VERSION 3.16)
project(clubins_host C)

set(CMAKE_C_STANDARD 11)

include(${CMAKE_CURRENT_LIST_DIR}/../caps.cmake)

# The composition under test + the core it consumes.
set(HOST_SOURCES
    "${CLUBINS_APP_DIR}/components/clubins_core/src/measurement.c"
    "${CLUBINS_APP_DIR}/components/clubins_core/src/registry.c"
    "${CLUBINS_APP_DIR}/components/clubins_core/src/http_json.c"
    "${CLUBINS_APP_DIR}/main/app_compose.c"
)
set(HOST_INCLUDE_DIRS
    "${CLUBINS_APP_DIR}/components/clubins_core/include"
    "${CLUBINS_APP_DIR}/main"
)

# The flag-selected component sources + their include dirs (caps.cmake
# mirrors the Kconfig select graph the flag-matrix script hands in).
clubins_caps_host_sources(CLUBINS_CAP_SOURCES CLUBINS_CAP_INCLUDE_DIRS)
list(APPEND HOST_SOURCES ${CLUBINS_CAP_SOURCES})
list(APPEND HOST_INCLUDE_DIRS ${CLUBINS_CAP_INCLUDE_DIRS})

# Vendored Unity — kept out of the -Werror surface.
add_library(unity STATIC unity/unity.c)
target_include_directories(unity PUBLIC ${CMAKE_CURRENT_LIST_DIR}/unity)

add_executable(test_http_json_codec test_http_json_codec.c ${HOST_SOURCES})
target_include_directories(test_http_json_codec PRIVATE ${HOST_INCLUDE_DIRS})

add_executable(test_ingest_conformance test_ingest_conformance.c ${HOST_SOURCES})
target_include_directories(test_ingest_conformance PRIVATE ${HOST_INCLUDE_DIRS})
target_compile_definitions(test_ingest_conformance
    PRIVATE CLUBINS_FIXTURE_DIR="${CMAKE_CURRENT_LIST_DIR}/fixtures/ingest"
)

add_executable(test_registry_compose test_registry_compose.c ${HOST_SOURCES})
target_include_directories(test_registry_compose PRIVATE ${HOST_INCLUDE_DIRS})

foreach(target test_http_json_codec test_ingest_conformance test_registry_compose)
    target_compile_options(${target} PRIVATE -Wall -Wextra -Werror)
    target_link_libraries(${target} PRIVATE unity)
endforeach()

enable_testing()
add_test(NAME http_json_codec COMMAND test_http_json_codec)
add_test(NAME ingest_conformance COMMAND test_ingest_conformance)
add_test(NAME registry_compose COMMAND test_registry_compose)
