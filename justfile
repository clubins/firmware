# Clubins firmware — host CI gate and ESP32-S3 build recipes.
# Run inside the pinned toolchain shell: nix-shell --pure --run 'just check'

set shell := ["bash", "-euo", "pipefail", "-c"]

xtensa := "xtensa-esp32s3-none-elf"

[default]
default:
    @{{ just_executable() }} --list

# Everything host CI runs: format, clippy -D warnings, host tests, Xtensa check.
check: fmt lint test xtensa-check

# cargo dispatches external subcommands (fmt, clippy) from $CARGO_HOME/bin
# before its own directory — a host rustup shim there (present on CI runner
# images) honors rust-toolchain.toml and dies on our custom fork toolchain.
# Point those recipes at a clean CARGO_HOME so the pinned toolchain's binaries
# win. Built-in subcommands (build/check/test) never dispatch externally and
# keep the shared registry cache.
fmt:
    CARGO_HOME="$(mktemp -d)" cargo fmt --all --check

# Lint gate.
lint:
    CARGO_HOME="$(mktemp -d)" cargo clippy --locked -- -D warnings

# Host tests for the pure-logic modules.
test:
    cargo test --locked

# Type-check the device personality for the ESP32-S3 (no link, no GCC).
xtensa-check:
    cargo check --locked --target {{ xtensa }} -Zbuild-std=core

# Link a release image for the ESP32-S3 with no features (all-off baseline).
xtensa-build-off:
    cargo build --release --locked --no-default-features --target {{ xtensa }} -Zbuild-std=core

# Link a release image for the ESP32-S3 with all features enabled.
xtensa-build-all:
    cargo build --release --locked --all-features --target {{ xtensa }} -Zbuild-std=core

# Compile-verify the flag matrix on the host: all-off, each leaf flag alone
# (15 runs), all-on. Each combo also runs its unit tests, so the feature-flag
# resolution test is exercised under every combination.
flag-matrix:
    #!/usr/bin/env bash
    set -euo pipefail
    leaf_flags=(
      sensor-decibel sensor-pax sensor-bme280 sensor-sht4x sensor-scd4x
      sensor-pms5003 sensor-bh1750 transport-http transport-mqtt
      transport-mtls ota provisioning deep-sleep logging-defmt ops-status
    )
    combos=("--no-default-features" "--all-features")
    for flag in "${leaf_flags[@]}"; do
      combos+=("--no-default-features --features $flag")
    done
    for combo in "${combos[@]}"; do
      echo ":: flag matrix: cargo check + test $combo"
      cargo check --locked $combo
      cargo test --locked $combo
    done
