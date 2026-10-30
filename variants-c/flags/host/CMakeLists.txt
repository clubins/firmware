# Host harness for the platform-agnostic core: CTest + Unity.
#
# Builds the same cap_registry.c and stub sources the ESP-IDF app compiles
# (the mapping lives in caps.cmake, shared verbatim), driven by the port of
# V1's tests/flag_framework.rs. The flag combination under test comes in as
# -DCONFIG_CLUBINS_CAP_*=1 definitions (tools/flag-matrix.sh mirrors the
# Kconfig select graph); anything unset reads as off via caps_config.h —
# which is why no sdkconfig.h is needed here.

cmake_minimum_required(VERSION 3.16)
project(clubins_flags_host C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

include(${CMAKE_CURRENT_LIST_DIR}/../caps.cmake)

set(CLUBINS_UNITY_DIR "${CLUBINS_APP_DIR}/host/unity")

# The core under test — strict warnings are the C analog of clippy
# -D warnings. Unity (third party) is built outside this target so its
# warnings never mask ours.
add_library(clubins_core STATIC
    "${CLUBINS_APP_DIR}/main/cap_registry.c"
    ${CLUBINS_CAP_SOURCES}
)
target_include_directories(clubins_core PUBLIC "${CLUBINS_APP_DIR}/main")
target_compile_options(clubins_core PRIVATE -Wall -Wextra -Werror)

add_executable(test_flag_framework
    test_flag_framework.c
    "${CLUBINS_UNITY_DIR}/unity.c"
)
target_include_directories(test_flag_framework PRIVATE "${CLUBINS_UNITY_DIR}")
target_link_libraries(test_flag_framework PRIVATE clubins_core)

enable_testing()
add_test(NAME flag_framework COMMAND test_flag_framework)
