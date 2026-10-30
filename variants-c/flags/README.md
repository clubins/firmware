# C1 — capability-flags firmware (ESP-IDF, Kconfig)

The C port of Rust V1's capability-flag framework: one ESP-IDF app in which
every capability sits behind a Kconfig toggle (`CONFIG_CLUBINS_CAP_*`), is
compiled in or out through CMake conditionals, and is dispatched at runtime
by a static registry. Capabilities that are not compiled in are absent —
no memory, no code path. This variant deliberately mirrors V1 (the root
Rust crate): its 15-flag matrix, its sensor/transport/ops layout, its
superloop-as-periodic-timer shape, and its flag-framework test suite.

Everything here is a scaffold stage of the real firmware: the framework,
the wiring, and the contracts are real; each capability's body is a stub
whose real implementation lands in its own task. A stub is an empty
`*_init`/`*_tick` pair with a header comment naming the task that will
replace it — never a `// TODO` in the dispatch path.

## ESP-IDF pin

**ESP-IDF v5.5.6** — the stable 5.x release, pinned across all `variants-c/`
variants. Recorded here and in the CI workflow (the same
`espressif/esp-idf-ci-action` version input).

## Layout

```
variants-c/flags/
├── CMakeLists.txt            ESP-IDF project entry (idf_build_process)
├── caps.cmake                flag → source mapping (shared verbatim by the
│                             host harness — one source of truth)
├── Kconfig.projbuild         15 public capability flags + 5 plumbing flags
├── sdkconfig.defaults        base device defaults (all-off capabilities)
├── sdkconfig.defaults.all-features
│                             all-15 anchor for the device CI job
├── main/
│   ├── CMakeLists.txt        registers the main component
│   ├── idf_component.yml     (empty requirements; explicit for reviewers)
│   ├── main.c               boot init + FreeRTOS timer loop (the device
│   │                         mirror of V1's superloop)
│   ├── caps_config.h         CONFIG_* → plain 0/1 macros (platform-agnostic)
│   ├── cap_registry.h/.c     static module/sensor registry + dispatcher
│   └── stubs/
│       ├── stubkit.h         stub export contract
│       ├── sensors/          decibel · pax · bme280 · sht4x · scd4x ·
│       │                     pms5003 · bh1750
│       ├── transports/       http · mqtt
│       └── ops/              mtls · ota · ble_provisioning · deep_sleep ·
│                             logging_defmt · status
├── host/
│   ├── CMakeLists.txt        CTest + Unity harness (platform-agnostic core)
│   ├── test_flag_framework.c the port of V1's tests/flag_framework.rs
│   └── unity/                vendored Unity 2.x (from ESP-IDF's copy)
├── tools/
│   └── flag-matrix.sh        17-combination matrix gate (host + device)
├── .clang-format             LLVM base, 4-space indent, 100 columns
└── .gitignore                build/, sdkconfig, managed_components/
```

## Flag matrix (the port of V1's 15 features)

Every public leaf is a Kconfig symbol; plumbing flags are internal
(dependency carriers only). `select` mirrors V1's cargo feature
dependencies.

| Kconfig leaf (CONFIG_CLUBINS_CAP_*) | Compiles in | Selects | Wire type |
|---|---|---|---|
| `SENSOR_DECIBEL` | SPK0641HT4H-1 PDM mic front-end + DSP (I2S0 PDM 48 kHz, A/C weighting, LAeq/LCeq Fast extrema, overload, per-unit calibration) | — | `decibel` |
| `SENSOR_PAX` | GPIO PIR occupancy front-end | `DRIVER_GPIO` | `pax` |
| `SENSOR_BME280` | temperature + humidity + pressure | `DRIVER_I2C` | `weather` |
| `SENSOR_SHT4X` | temperature + humidity | `DRIVER_I2C` | `weather` |
| `SENSOR_SCD4X` | CO₂ (new type) | `DRIVER_I2C` | `co2` |
| `SENSOR_PMS5003` | PM2.5 particulates (new type) | `DRIVER_UART` | `pm25` |
| `SENSOR_BH1750` | ambient light (new type) | `DRIVER_I2C` | `light` |
| `TRANSPORT_HTTP` | Wi-Fi + HTTPS JSON ingest (working server path) | `NET_WIFI` | — |
| `TRANSPORT_MQTT` | MQTT v2 protobuf client per `api/mqtt` | `NET_WIFI`, `PROTO_V2` | — |
| `TRANSPORT_MTLS` | client certs from NVS (CN = sensor UUID) | `TRANSPORT_MQTT` | — |
| `OTA` | esp-ota update handling | — | — |
| `PROVISIONING` | BLE Wi-Fi credential provisioning | `NET_WIFI` | — |
| `DEEP_SLEEP` | duty-cycled sampling | — | — |
| `LOGGING_DEFMT` | structured logging transport | — | — |
| `OPS_STATUS` | status heartbeat + boot lifecycle | — | — |

Defaults are **all-off** — the C mirror of V1's `default = []`: a bare
build is the framework itself. The all-15 anchor lives in
`sdkconfig.defaults.all-features` and is what the device CI job builds
second. A hardware watchdog, once the ops task lands, will run
unconditionally (safety property, not a flag) — the C1 equivalent of V1's
decision.

New ingest types (`co2`, `pm25`, `light`) are emitted per the
one-topic-per-type rule; no server receiver exists yet — the gap is
tracked for the backend, not the firmware (spec §Decisions).

## How the dispatch works

`main/cap_registry.c` is the port of V1's `registry.rs`: every capability
arm is `#if`-guarded, so a build's registry is exactly the set of modules
CMake compiled in. Stubs export `clubins_stub_*_init`/`_tick`; registry
arms reference those symbols directly, so a flag wired to a module that
CMake did not compile is a link error at build time — never a null
callback at boot. `app_main.c` initializes the registry once, then drives
`clubins_registry_tick_all()` from a 1 s FreeRTOS software timer — the
periodic shape this architecture keeps for the real loop (sample → window
→ encode → publish).

Config precedence is planned as in V1: NVS value → committed defaults →
built-in constant, with secrets (Wi-Fi credentials, API key, client cert)
only ever in NVS. The committed defaults land with the config task.

## Verification (this scaffold)

| Gate | Command | What proves |
|---|---|---|
| Host CTest | `tools/flag-matrix.sh` (host mode) | flag resolution, module presence, stub lifecycle — under all 17 combinations |
| Flag matrix (host) | same | all-off, each leaf alone (+ its Kconfig selects), all-on |
| Device build (default) | `idf.py build` | the ESP32-S3 app links with capabilities off |
| Device build (all features) | `tools/flag-matrix.sh --device` or the CI job | the ESP32-S3 app links with all 15 on |
| Format | `clang-format --dry-run -Werror` (CI) | tree is format-clean |

The flag-framework test suite is the port of V1's
`tests/flag_framework.rs`: `feature_flag_resolution` (the registry must
mirror the Kconfig selection of the build), `module_presence` (unique ids,
contract-exact wire types, disabled = absent), `stub_lifecycle` (hooks
safe to drive repeatedly). It runs under every combination of the matrix,
so a mis-wired flag fails in the combination that broke it.

## Local development

With the ESP-IDF v5.5.6 environment on your PATH (or inside the
`espressif/idf:v5.5.6` container):

```sh
# Host: platform-agnostic core, one combination at a time
cmake -S host -B build/host && cmake --build build/host && ctest --test-dir build/host

# Host: the whole 17-combination matrix
tools/flag-matrix.sh

# Device: default build
idf.py -B build.default set-target esp32s3 build

# Device: one full matrix (17 idf.py builds — slow but total)
tools/flag-matrix.sh --device
```

CI (`.github/workflows/ci-c1.yml`, GitHub-hosted `ubuntu-latest`) runs the
format check, the full host matrix, and two `espressif/esp-idf-ci-action`
builds (default + all-features) against ESP-IDF v5.5.6.

## V1 → C1 porting map

| V1 (root crate, Rust) | C1 (this directory, C) |
|---|---|
| `Cargo.toml` `[features]` | `Kconfig.projbuild` `CONFIG_CLUBINS_CAP_*` |
| `#[cfg(feature = …)]` arms | CMake `if(CONFIG_…)` + `#if` guards |
| `src/registry.rs` | `main/cap_registry.c` |
| `src/sensors/*.rs` | `main/stubs/sensors/*.c` |
| `src/transport/{http,mqtt}.rs` | `main/stubs/transports/*.c` |
| `src/ops.rs` | `main/stubs/ops/*.c` |
| superloop in `main.rs` | FreeRTOS software timer in `main/main.c` |
| `tests/flag_framework.rs` | `host/test_flag_framework.c` (Unity) |
| `just flag-matrix` | `tools/flag-matrix.sh` |

Isolation note: this variant is self-contained under `variants-c/flags/`
by design — it shares no build state with the root Rust crate or the
`variants/` Rust directories, and nothing outside this directory is
needed to build or test it.
