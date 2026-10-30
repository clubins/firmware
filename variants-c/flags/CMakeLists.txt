# Clubins C1 (flags) — single ESP-IDF application for the ESP32-S3.
#
# The C port of Rust V1's shape: one project, two personalities. The
# capability framework (Kconfig flags, registry, stubs) is platform-agnostic
# C with no IDF headers and recompiles on the host under host/ for
# CTest + Unity; this root file drives the device personality only.
#
# ESP-IDF pin: v5.5.6 — shared by every variants-c/ project (recorded in
# README.md and the CI workflow; change it in all three places together).
cmake_minimum_required(VERSION 3.16)
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(clubins_flags)
