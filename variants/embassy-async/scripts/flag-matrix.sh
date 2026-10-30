#!/usr/bin/env bash
# Flag-matrix verification: all-off, every public leaf alone, all-on.
# Runs inside nix-shell --pure (see the justfile). Every combination must
# compile; the script fails at the first one that does not.
set -euo pipefail

leaves=(
  sensor-decibel sensor-pax sensor-bme280 sensor-sht4x sensor-scd4x
  sensor-pms5003 sensor-bh1750
  transport-http transport-mqtt transport-mtls
  ota provisioning deep-sleep logging-defmt ops-status
)

echo "== matrix: no default features =="
cargo build --locked --no-default-features

for flag in "${leaves[@]}"; do
  echo "== matrix: --no-default-features --features $flag =="
  cargo build --locked --no-default-features --features "$flag"
done

echo "== matrix: --all-features =="
cargo build --locked --all-features

echo "flag matrix OK: ${#leaves[@]} leaves + all-off + all-on"
