# clubins/firmware

Clubins venue-sensor firmware for the ESP32-S3 (Rust, esp-hal 1.2.2, no_std).

Every capability — sensors, transports, operations — sits behind a Cargo feature flag so each device image only contains what it needs. See [docs/spec](docs/spec/) for the full specification.

## Status

- **Spec:** approved — full-feature flagged build covering 15 capability flags, seven sensor driver families, HTTP + MQTT v2 transports, config/NVS, mTLS, OTA, BLE provisioning, deep sleep, defmt diagnostics, and status reporting.
- **Implementation:** in progress. The plan is a 17-PR dependency DAG ([docs/plan/build-dag.md](docs/plan/build-dag.md)); this README will list shipped features as PRs merge.
- **Shipped:** the crate scaffold with the full 15-flag capability framework — stub modules behind every flag, pinned toolchain, hosted-runner CI with a flag-matrix job. Drivers, transports, and the config model land in the downstream PRs.

## Feature flags

`default = []` — an image carries exactly what its flags select. Any combination must compile; CI verifies all-off, each flag alone, and all-on.

| Flag | What it compiles in | Status |
|---|---|---|
| `sensor-decibel` | SPK0641HT4H-1 PDM mic front-end + DSP pipeline (per `decibel.md`); emits `decibel` (dba, dbc) | stub |
| `sensor-pax` | GPIO PIR occupancy front-end; emits `pax` (occupancy_count) | stub |
| `sensor-bme280` | Temperature, humidity, pressure → `weather` | stub |
| `sensor-sht4x` | Temperature, humidity → `weather` | stub |
| `sensor-scd4x` | CO₂ → `co2` (new measurement type) | stub |
| `sensor-pms5003` | PM2.5 → `pm25` (new measurement type) | stub |
| `sensor-bh1750` | Ambient light → `light` (new measurement type) | stub |
| `transport-http` | Wi-Fi + HTTPS JSON ingest (working server path) | stub |
| `transport-mqtt` | MQTT v2 protobuf client per the `api/mqtt` design | stub |
| `transport-mtls` | mTLS client certs from NVS (CN = sensor UUID) | stub |
| `ota` | esp-ota update handling | stub |
| `provisioning` | BLE Wi-Fi credential provisioning | stub |
| `deep-sleep` | Duty-cycled sampling between sleeps | stub |
| `logging-defmt` | defmt logging transport | stub |
| `ops-status` | Status heartbeat + boot lifecycle | stub |

Internal plumbing flags (`driver-i2c`, `driver-uart`, `driver-gpio`, `net-wifi`, `proto-v2`) keep dependency graphs honest and are not part of the public matrix.

## Layout

| Path | Purpose |
|---|---|
| `src/main.rs` | no_std esp-hal app entry (Xtensa only; host builds get a std stub) |
| `src/lib.rs` | Host-testable core: module tree, feature-gated stubs, the two layout tests |
| `src/sensor.rs`, `src/registry.rs` | Sensor identity + feature-flag resolution |
| `src/config.rs`, `src/state.rs` | Placeholder anchors for the config and ops-status PRs |
| `src/sensors/`, `src/transports/`, `src/ops/` | One stub module per capability flag |
| `config/default.toml` | Committed runtime defaults (placeholder until the config PR; secrets live only in NVS) |
| `.cargo/config.toml` | Xtensa linking: rust-lld + `-Tlinkall.x` |
| `esp-toolchain.nix` / `shell.nix` | Pinned Xtensa Rust fork 1.97.0.0 + GCC 15.2.0_20250920 dev/CI shell |
| `justfile` | Host CI gate (`just check`), Xtensa builds, flag-matrix verification |

## Build and check

Prerequisite: [Nix](https://nixos.org/download). The shell pins everything else (toolchain, just, espflash).

```bash
nix-shell --pure            # pinned toolchain shell

just check                  # fmt + clippy -D warnings + host tests + Xtensa check
just test                   # host unit tests (feature-flag resolution, module presence)
just flag-matrix            # cargo check + test: all-off, each flag alone, all-on
just xtensa-build-off       # release ELF, no features
just xtensa-build-all       # release ELF, all features
```

The ESP32-S3 target links with rust-lld + esp-hal's linker scripts (`-Tlinkall.x`) + `-Zbuild-std=core`; the recipe and its gotchas are recorded in [docs/research/firmware-internals-build-and-ci.md](docs/research/firmware-internals-build-and-ci.md). `rust-toolchain.toml` documents the fork version for rustup users — the actual toolchain comes from the Nix shell, since the esp-rs fork is not rustup-installable.

## CI

GitHub-hosted `ubuntu-latest` runners (this repo has no self-hosted NixOS runner). Every job installs Nix via the Determinate Systems installer, then runs inside `nix-shell --pure`:

- **host check** — `just check`: fmt, clippy `-D warnings`, host tests, Xtensa `cargo check`.
- **flag matrix** — `just flag-matrix`: 17 combinations (all-off, each leaf flag alone, all-on), each compiled and unit-tested.
- **xtensa** — both release images link for `xtensa-esp32s3-none-elf`: all-off and all-features.

Cargo registry downloads are cached between runs; the Nix toolchain shell is rebuilt deterministically each run.

## Documentation

| Document | Contents |
|---|---|
| [docs/spec/clubins-firmware-full-feature-flagged-build-spec.html](docs/spec/clubins-firmware-full-feature-flagged-build-spec.html) | Approved build spec: problem frame, current state, architecture, feature-flag matrix, sensor expansion, config model, verification criteria, decisions and risks |
| [docs/research/firmware-internals-build-and-ci.md](docs/research/firmware-internals-build-and-ci.md) | Evidence base: repo state at research time, proven ESP32-S3 build recipe (rust-lld + linker scripts + `-Zbuild-std=core`), CI contract, PR conventions, flag-framework constraints |
| [docs/research/integration-surface-data-contracts.md](docs/research/integration-surface-data-contracts.md) | Backend contracts the firmware must fulfill: HTTP JSON ingest (exact Go struct fields), MQTT v2 protobuf design, hardware inventory, dashboard needs |
| [docs/plan/build-dag.md](docs/plan/build-dag.md) | Execution plan: 17 PR-sized tasks, dependency waves, complexity, and per-task verification |

## Conventions

- Conventional Commits (`feat:`, `fix:`, `test:`, `chore:`, `docs:`); rustfmt edition 2024, max_width 90 (`cargo fmt`).
- One capability = one Cargo feature; sensor drivers emit typed measurements, one topic per measurement type.
