# Clubins firmware — variant C (embassy-async)

ESP32-S3 sensor firmware in the embassy-async architecture: one async task per
concern, cooperating over statically bounded typed channels. Every capability
is a cargo feature; every runtime tunable is a config value. Nothing is
hard-wired — deciding on features later is a config edit, never a rewrite.

This is one of three parallel architecture variants in this repo (V1 root
monolith, V2 multi-crate workspace, V3 this one). All satisfy the same
backend contracts.

## Concurrency model (what makes this variant V3)

No superloop. `embassy-executor` + `embassy-time` on esp-hal 1.2.2 (driven by
`esp-rtos`), independent async tasks per concern:

```text
 sensor tasks (per flag)          ops tasks
 ┌───────────────────┐            ┌──────────────┐
 │ decibel  ┐        │            │ ops-status   │──┐
 │ pax      │ publish│            │ ota          │  │ publish
 │ bme280   ┘        │            │ provisioning │  │
 └────────┬──────────┘            │ deep-sleep   │  │
          │ send().await          └──────┬───────┘  │
          ▼                              ▼          ▼
 ┌──────────────────────────────────────────────────────┐
 │ MeasurementQueue (16)         StatusQueue (8)        │  ← bounded, static
 └──────────────────────────────┬───────────────────────┘
                                ▼ receive()
                     ┌─────────────────────┐
                     │ active transport    │  HTTP JSON (default)
                     │ (one at runtime)    │  or MQTT v2 protobuf
                     └─────────────────────┘
```

- **Backpressure, not loss:** `send().await` blocks the producer when a queue
  is full. A stalled transport slows sampling; nothing is dropped or buffered
  unboundedly. Queue depths are `src/config.rs` constants.
- **Task isolation:** a panicked sensor task cannot block a transport; the
  executor schedules the rest. A non-active transport task exits immediately,
  so each event reaches exactly one transport.

## Task map

| Task (in `src/device/tasks.rs`) | Feature flag      | Channel endpoint        |
| ------------------------------- | ----------------- | ----------------------- |
| `decibel_sensor_task`           | `sensor-decibel`  | measurement sender      |
| `pax_sensor_task`               | `sensor-pax`      | measurement sender      |
| `bme280_sensor_task`            | `sensor-bme280`   | measurement sender      |
| `sht4x_sensor_task`             | `sensor-sht4x`    | measurement sender      |
| `scd4x_sensor_task`             | `sensor-scd4x`    | measurement sender      |
| `pms5003_sensor_task`           | `sensor-pms5003`  | measurement sender      |
| `bh1750_sensor_task`            | `sensor-bh1750`   | measurement sender      |
| `http_transport_task`           | `transport-http`  | both receivers          |
| `mqtt_transport_task`           | `transport-mqtt`  | both receivers          |
| `ota_task`                      | `ota`             | —                       |
| `provisioning_task`             | `provisioning`    | —                       |
| `deep_sleep_task`               | `deep-sleep`      | —                       |
| `ops_status_task`               | `ops-status`      | status sender           |

`features::planned_tasks()` is the tested mirror of the spawn arms in
`src/device/mod.rs`; `logging-defmt` selects a logging transport, not a task.
When both transports are compiled, `config/default.toml` `[transport] active`
picks the one that runs (`features::resolve_transport`, host-tested).

## Feature flags

Public capabilities (the flag matrix — 15):

| Flag              | Compiles in                                    | Depends on    |
| ----------------- | ---------------------------------------------- | ------------- |
| `sensor-decibel`  | PDM mic front-end + DSP (per `decibel.md`)     | —             |
| `sensor-pax`      | GPIO PIR occupancy front-end                   | `driver-gpio` |
| `sensor-bme280`   | temperature + humidity + pressure → weather    | `driver-i2c`  |
| `sensor-sht4x`    | temperature + humidity → weather               | `driver-i2c`  |
| `sensor-scd4x`    | CO₂ → co2 (new ingest type)                    | `driver-i2c`  |
| `sensor-pms5003`  | PM2.5 → pm25 (new ingest type)                 | `driver-uart` |
| `sensor-bh1750`   | ambient light → light (new ingest type)        | `driver-i2c`  |
| `transport-http`  | Wi-Fi + HTTPS JSON ingest (working server path)| `net-wifi`    |
| `transport-mqtt`  | MQTT v2 protobuf client per `api/mqtt`         | `net-wifi`, `proto-v2` |
| `transport-mtls`  | mTLS client certs from NVS (CN = sensor UUID)  | `transport-mqtt` |
| `ota`             | esp-ota update handling                        | —             |
| `provisioning`    | BLE Wi-Fi credential provisioning              | `net-wifi`    |
| `deep-sleep`      | duty-cycled sampling                           | —             |
| `logging-defmt`   | defmt logging transport                        | —             |
| `ops-status`      | status heartbeat + boot lifecycle (default on) | —             |

Internal plumbing flags (`driver-i2c`, `driver-uart`, `driver-gpio`,
`net-wifi`, `proto-v2`) are dependency edges only — not part of the public
matrix and not runtime capabilities.

Defaults: `sensor-decibel`, `transport-http`, `ops-status` — the set the
server can receive today.

## Two personalities, one crate

- **Library** (`src/lib.rs`, `no_std`): the event model, the typed bounded
  channels, config constants and feature resolution. Pure logic, compiles and
  tests on the host — CI stays fast.
- **Device** (`src/main.rs` + `src/device/`, Xtensa only): boots esp-hal,
  starts `esp-rtos`, spawns the planned tasks. Every hardware dependency is
  scoped to `cfg(target_arch = "xtensa")`, so the host personality never
  compiles esp-hal.

## Runtime configuration

`config/default.toml` is the canonical defaults file (committed, no secrets —
they live in NVS). Precedence rule: NVS override > `default.toml` >
built-in constant. Until the config task wires the file-backed tiers,
`src/config.rs` carries the same values and names the keys they stand in.

## Build and verify

All recipes enter the pinned toolchain shell themselves:

```sh
just check         # fmt + clippy -D warnings + host build + host tests
just flag-matrix   # all-off, each of the 15 leaves alone, all-on (host)
just xtensa        # release ELF for xtensa-esp32s3-none-elf
just xtensa --all-features
```

Or directly: `nix-shell --pure --run 'just check'`.

The toolchain (`shell.nix` + `esp-toolchain.nix`) pins the CI-exact stack:
Rust fork 1.97.0.0, Xtensa GCC 15.2.0. The link recipe is `rust-lld` +
`-Tlinkall.x` (`-Zbuild-std=core` via `CARGO_UNSTABLE_BUILD_STD`); the GCC
driver cannot link the cross-endian ELF, and missing esp-hal linker scripts
fail on `R_XTENSA_SLOT0_OP` relocations. There is deliberately **no**
`rust-toolchain.toml` — the Nix shell is the toolchain manager.

## CI

`.github/workflows/ci-v3.yml` (this variant's own workflow — the repo has no
self-hosted runner):

- **host check** — `just check` (fmt, clippy `-D warnings`, build, test).
- **flag matrix** — `just flag-matrix` (17 host builds).
- **xtensa** — release ELF for default and all-features, uploaded as
  artifacts.

## Scaffold status

Real and verified: executor boot + task spawning, typed bounded channels with
backpressure, the full event model (contract-shaped), feature resolution and
task planning, the ops-status lifecycle events, runtime transport selection.

Deliberately skeleton (each arrives as its own flag-scoped PR, exactly what
the framework is for): sensor drivers and the decibel DSP, the Wi-Fi/TLS
stacks and both transport encoders, OTA/provisioning/deep-sleep bodies, NVS
config loading. Skeletons never fabricate measurements.

Verification is compile-level plus host test suites — no physical board is
flashed in this build.

## Isolation rules

All toolchain files live inside this directory (nothing at repo root); CI and
builds run from here. This variant never touches the repo-root crate (V1),
`variants/workspace/` (V2), or `docs/`.
