#!/usr/bin/env bash
# Flag-matrix gate for the C1 flags variant — the C port of V1's
# `just flag-matrix`: all-off, each public leaf alone (together with the
# plumbing its Kconfig `select`s pull in), all-on — 17 combinations.
#
# Default mode runs the host harness (CTest + Unity) over the platform-
# agnostic core: seconds per combination, so CI runs all 17 in one job.
# --device mode builds the real ESP-IDF project per combination via idf.py
# with a generated per-combination sdkconfig defaults file — run it locally
# (or on a beefed-up runner); 17 full IDF builds are too slow for a hosted
# CI job, which instead anchors default + all-features device builds.
#
# Usage:
#   tools/flag-matrix.sh                  # host: 17 × (cmake configure + build + ctest)
#   tools/flag-matrix.sh --device         # device: 17 × idf.py build (needs IDF v5.5.6)
#   tools/flag-matrix.sh --one SENSOR_PAX # a single combination (host, for debugging)

set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="${CLUBINS_MATRIX_BUILD_DIR:-$APP_DIR/build/matrix}"

# The 15 public leaves in stable registry order (cap_registry.h).
LEAVES=(
    SENSOR_DECIBEL SENSOR_PAX SENSOR_BME280 SENSOR_SHT4X SENSOR_SCD4X
    SENSOR_PMS5003 SENSOR_BH1750 TRANSPORT_HTTP TRANSPORT_MQTT
    TRANSPORT_MTLS OTA PROVISIONING DEEP_SLEEP LOGGING_DEFMT OPS_STATUS
)

# Kconfig select graph (Kconfig.projbuild): leaf → flags it pulls in. This
# mirrors V1's cargo feature dependencies; TRANSPORT_MTLS → TRANSPORT_MQTT
# expands transitively (NET_WIFI, PROTO_V2) below.
kconfig_selects() {
    case "$1" in
    SENSOR_PAX) echo "DRIVER_GPIO" ;;
    SENSOR_BME280 | SENSOR_SHT4X | SENSOR_SCD4X | SENSOR_BH1750) echo "DRIVER_I2C" ;;
    SENSOR_PMS5003) echo "DRIVER_UART" ;;
    TRANSPORT_HTTP) echo "NET_WIFI" ;;
    TRANSPORT_MQTT) echo "NET_WIFI PROTO_V2" ;;
    TRANSPORT_MTLS) echo "TRANSPORT_MQTT" ;;
    PROVISIONING) echo "NET_WIFI" ;;
    *) echo "" ;;
    esac
}

# Expand a flag list until its Kconfig selects are closure-complete;
# prints one flag per line.
expand_flags() {
    local flags=("$@") changed=1
    while [ "$changed" -eq 1 ]; do
        changed=0
        for flag in "${flags[@]}"; do
            for dep in $(kconfig_selects "$flag"); do
                local known=0
                for existing in "${flags[@]}"; do
                    if [ "$existing" = "$dep" ]; then known=1; break; fi
                done
                if [ "$known" -eq 0 ]; then
                    flags+=("$dep")
                    changed=1
                fi
            done
        done
    done
    printf '%s\n' "${flags[@]}"
}

# Host combination: cmake configure + build + ctest with the combination's
# CONFIG_CLUBINS_CAP_* definitions.
run_host() {
    local name="$1"
    shift
    local defines=()
    for flag in "$@"; do
        defines+=("-DCONFIG_CLUBINS_CAP_${flag}=1")
    done
    local build="$BUILD_DIR/host-$name"
    cmake -S "$APP_DIR/host" -B "$build" -DCMAKE_BUILD_TYPE=Release \
        "${defines[@]}" > /dev/null
    cmake --build "$build" --parallel > /dev/null
    ctest --test-dir "$build" --output-on-failure > /dev/null
    echo "ok: $name"
}

# Device combination: idf.py build with a generated per-combination
# sdkconfig defaults file (the base defaults plus the combination's y
# lines; Kconfig resolves the select graph itself).
run_device() {
    local name="$1"
    shift
    local combo_dir="$BUILD_DIR/device-$name"
    mkdir -p "$combo_dir"
    {
        cat "$APP_DIR/sdkconfig.defaults"
        for flag in "$@"; do
            echo "CONFIG_CLUBINS_CAP_${flag}=y"
        done
    } > "$combo_dir/sdkconfig.defaults.combo"
    # SDKCONFIG_DEFAULTS is a semicolon-separated list, paths relative to
    # the project dir; keep every combination in its own build dir so the
    # generated sdkconfig never leaks between combos.
    if ! (cd "$APP_DIR" && idf.py -B "$combo_dir/build" \
        -D "SDKCONFIG_DEFAULTS=sdkconfig.defaults;$combo_dir/sdkconfig.defaults.combo" \
        build > "$combo_dir/build.log" 2>&1); then
        tail -40 "$combo_dir/build.log"
        echo "FAILED: $name (device)" >&2
        return 1
    fi
    echo "ok: $name (device)"
}

MODE=host
ONE=""
case "${1:-}" in
"") ;;
--device) MODE=device ;;
--one)
    MODE=one
    ONE="${2:?--one needs a capability name, e.g. SENSOR_PAX}"
    ;;
*)
    echo "usage: $0 [--device | --one CAP_SUFFIX]" >&2
    exit 2
    ;;
esac

echo "== all-off =="
case "$MODE" in
host) run_host "all-off" ;;
device) run_device "all-off" ;;
one) echo "--one runs a single leaf combination; all-off has no flag" >&2; exit 2 ;;
esac

for leaf in "${LEAVES[@]}"; do
    if [ "$MODE" = "one" ] && [ "$leaf" != "$ONE" ]; then continue; fi
    echo "== alone: $leaf =="
    mapfile -t combo < <(expand_flags "$leaf")
    case "$MODE" in
    host | one) run_host "$leaf" "${combo[@]}" ;;
    device) run_device "$leaf" "${combo[@]}" ;;
    esac
done

echo "== all-on =="
case "$MODE" in
host | one) run_host "all-on" "${LEAVES[@]}" ;;
device) run_device "all-on" "${LEAVES[@]}" ;;
esac

echo "flag matrix green: 17 combinations ($MODE)"
