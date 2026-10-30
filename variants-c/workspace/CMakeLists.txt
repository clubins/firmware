# Clubins C2 (workspace) — the ESP32-S3 firmware as an ESP-IDF
# multi-component project: the C port of Rust V2's workspace-composed
# crates. One component per concern lives under components/ (the shared
# core API, one per driver, the transports, the ops capabilities); main is
# the slim composition root — the port of V2's firmware bin. The Kconfig
# capability selection decides which components enter the app's requirement
# graph; components that no link edge pulls in are pruned at link time.
#
# The platform-agnostic core recompiles on the host under host/ for
# CTest + Unity; this root file drives the device personality only.
#
# ESP-IDF pin: v5.5.6 — shared by every variants-c/ project (recorded in
# README.md and the CI workflow; change it in all three places together).
cmake_minimum_required(VERSION 3.16)

# Components live in concern groups (the mirror of V2's workspace member
# groups: firmware-core → clubins_core, drivers/* → drv_*, transports/* →
# transport_*, ops/* → ops_*/logging_defmt, net/* → net_wifi/proto_v2).
# ESP-IDF's default component scan is one level deep under components/, so
# the grouped components are registered explicitly here — the device mirror
# of caps.cmake's flag→component mapping. clubins_core sits at the top
# level and is found by the default scan.
set(EXTRA_COMPONENT_DIRS
    "${CMAKE_CURRENT_LIST_DIR}/components/drivers/drv_decibel"
    "${CMAKE_CURRENT_LIST_DIR}/components/drivers/drv_pax"
    "${CMAKE_CURRENT_LIST_DIR}/components/drivers/drv_bme280"
    "${CMAKE_CURRENT_LIST_DIR}/components/drivers/drv_sht4x"
    "${CMAKE_CURRENT_LIST_DIR}/components/drivers/drv_scd4x"
    "${CMAKE_CURRENT_LIST_DIR}/components/drivers/drv_pms5003"
    "${CMAKE_CURRENT_LIST_DIR}/components/drivers/drv_bh1750"
    "${CMAKE_CURRENT_LIST_DIR}/components/net/net_wifi"
    "${CMAKE_CURRENT_LIST_DIR}/components/net/proto_v2"
    "${CMAKE_CURRENT_LIST_DIR}/components/transports/transport_http"
    "${CMAKE_CURRENT_LIST_DIR}/components/transports/transport_mqtt"
    "${CMAKE_CURRENT_LIST_DIR}/components/ops/ops_ota"
    "${CMAKE_CURRENT_LIST_DIR}/components/ops/ops_provisioning"
    "${CMAKE_CURRENT_LIST_DIR}/components/ops/ops_sleep"
    "${CMAKE_CURRENT_LIST_DIR}/components/ops/ops_status"
    "${CMAKE_CURRENT_LIST_DIR}/components/ops/logging_defmt"
)

include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(clubins_workspace)
