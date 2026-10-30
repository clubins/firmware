# The composition root — the C port of V2's firmware bin crate.
#
# V2's firmware/Cargo.toml maps every spec §Feature Matrix flag to an
# optional workspace dependency; this file is the same mapping in the
# component world: each capability leaf pulls its component into the app's
# requirement graph. A flag wired to a component that is missing from
# caps.cmake's mapping is a link error in the combination that broke it —
# the flag matrix catches it. The mapping itself lives in caps.cmake,
# shared verbatim with the host harness.
#
# main.c is the only translation unit with IDF headers; app_compose.c is
# the platform-agnostic half that the host harness compiles from the same
# bytes.

include(${CMAKE_CURRENT_LIST_DIR}/../caps.cmake)

# Every capability component is a static build-graph dependency of the
# composition root. ESP-IDF evaluates component CMakeLists before the
# sdkconfig exists (the Kconfig-to-CMake export lands later than component
# processing), so REQUIRES cannot be flag-conditional at registration time.
# Feature selection happens in C instead: app_compose.c compiles only the
# CONFIG_CLUBINS_CAP_* arms it was built for, and unreferenced component
# code is dropped at link time (static libs + --gc-sections).
set(CLUBINS_REQUIRES clubins_core ${CLUBINS_CAP_COMPONENTS})

idf_component_register(
    SRCS "main.c" "app_compose.c"
    INCLUDE_DIRS "."
    REQUIRES ${CLUBINS_REQUIRES}
    PRIV_REQUIRES freertos log
)
