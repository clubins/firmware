# Clubins firmware — V2 workspace (multi-crate)

Variant B of the three-way firmware architecture build: a strict multi-crate
Cargo workspace for the ESP32-S3 on esp-hal 1.2.2. One crate per capability;
cargo features map to optional workspace dependencies; link-time pruning and
per-crate tests replace the monolith's flag-stub approach.

## Crate map

| Crate | Path | Role |
|---|---|---|
| `firmware-core` | `firmware-core/` | no_std+alloc core: measurement model (one variant per ingest type), `Sensor` trait + registry, runtime config shape |
| `firmware` (bin) | `firmware/` | slim composition root: config in → compiled-and-enabled sensors + one transport → sampling loop |
| `driver-decibel` | `drivers/driver-decibel/` | SPK0641HT4H-1 PDM mic + decibel DSP pipeline (`sensor-decibel`) |
| `driver-pax` | `drivers/driver-pax/` | PIR occupancy front-end (`sensor-pax`) |
| `driver-bme280` | `drivers/driver-bme280/` | temp/humidity/pressure (`sensor-bme280`) |
| `driver-sht4x` | `drivers/driver-sht4x/` | temp/humidity (`sensor-sht4x`) |
| `driver-scd4x` | `drivers/driver-scd4x/` | CO₂ (`sensor-scd4x`, new type) |
| `driver-pms5003` | `drivers/driver-pms5003/` | PM2.5 (`sensor-pms5003`, new type) |
| `driver-bh1750` | `drivers/driver-bh1750/` | ambient light (`sensor-bh1750`, new type) |
| `net-wifi` | `net/net-wifi/` | Wi-Fi STA plumbing + redacted credential type |
| `proto-v2` | `net/proto-v2/` | MQTT v2 protobuf codec (`Header`, `Status`) per `api/mqtt` |
| `transport-http` | `transports/transport-http/` | HTTP JSON ingest (`transport-http`) |
| `transport-mqtt` | `transports/transport-mqtt/` | MQTT v2 transport + topic rules (`transport-mqtt`, `transport-mtls` adds the mTLS module) |
| `ops-ota` | `ops/ops-ota/` | esp-ota update lifecycle (`ota`) |
| `ops-provisioning` | `ops/ops-provisioning/` | BLE Wi-Fi provisioning (`provisioning`) |
| `ops-sleep` | `ops/ops-sleep/` | deep-sleep duty cycling (`deep-sleep`) |
| `ops-status` | `ops/ops-status/` | status heartbeats (`ops-status`, required by both contracts) |
| `logging-defmt` | `ops/logging-defmt/` | defmt logging facade (`logging-defmt`) |

## Flag → crate matrix

All 15 public flags from spec §Feature Matrix, mapped to optional deps in
`firmware/Cargo.toml`. The monolith's internal plumbing flags
(`driver-i2c`, `driver-uart`, `driver-gpio`, `net-wifi`, `proto-v2`) become
ordinary crate dependencies of the drivers/transports that need them — the
dependency graph is the feature graph, so there is nothing left to keep
honest.

| Flag | Enables |
|---|---|
| `sensor-decibel` | `dep:driver-decibel` |
| `sensor-pax` | `dep:driver-pax` |
| `sensor-bme280` | `dep:driver-bme280` |
| `sensor-sht4x` | `dep:driver-sht4x` |
| `sensor-scd4x` | `dep:driver-scd4x` |
| `sensor-pms5003` | `dep:driver-pms5003` |
| `sensor-bh1750` | `dep:driver-bh1750` |
| `transport-http` | `dep:transport-http` (brings `net-wifi` with it once the Wi-Fi bring-up lands) |
| `transport-mqtt` | `dep:transport-mqtt` (brings `proto-v2`) |
| `transport-mtls` | `transport-mqtt` (+ mTLS module in `transport-mqtt`) |
| `ota` | `dep:ops-ota` |
| `provisioning` | `dep:ops-provisioning` |
| `deep-sleep` | `dep:ops-sleep` |
| `logging-defmt` | `dep:logging-defmt` |
| `ops-status` | `dep:ops-status` |

Default: `sensor-decibel`, `transport-http`, `ops-status` — what the server
can receive today.

## Scaffolding status

This is the B1 scaffold: every crate exists, every feature resolves, and the
composition is feature-mapped — but capability crates are stubs with pinned
surfaces (`sample()` returns `None`), awaiting the driver/transport/DSP
tasks. What is real now: the measurement model and config defaults
(host-tested), the `Sensor`/registry contract, the protobuf wire codec for
`Header`/`Status` (round-trip tested against the proto field numbers), topic
builders and per-topic seq counters, secret hygiene (redacting Debug for
credentials, keys and client certs), and the OTA/provisioning/sleep state
models.

The Xtensa HAL comes in with the *target*, not a feature
(`[target.'cfg(target_arch = "xtensa")'.dependencies]`), so host builds of
every feature combination never compile esp-hal, and target builds always
do.

## Build & CI

Everything runs inside `nix-shell --pure` from this directory — the shell
provides exactly the CI-pinned toolchain (Rust fork 1.97.0.0 with the Xtensa
backend + GCC 15.2.0, from `esp-toolchain.nix`); the host's rustc is hidden
(`--pure` is load-bearing: stable rustc breaks esp-hal ≤ 1.2.2).

```sh
nix-shell --pure --run 'just check'    # fmt + clippy -D warnings + build + test
nix-shell --pure --run 'just feature-matrix'  # all-off, each flag alone, all-on
nix-shell --pure --run 'just xtensa'          # ESP32-S3 release image (default features)
nix-shell --pure --run 'just xtensa-all'      # ESP32-S3 release image (all features)
```

Xtensa linking uses rust-lld + `-Tlinkall.x` (`.cargo/config.toml`) and
`-Zbuild-std=core,alloc` (the fork ships no prebuilt std for the target).
GNU ld cannot link Xtensa relocations — do not switch linkers.

CI lives in `.github/workflows/ci-v2.yml` on GitHub-hosted `ubuntu-latest`
with the Determinate Systems Nix installer: the check job, the feature
matrix job, and the Xtensa link job (default and all features). Security
scans follow the monorepo's separate scheduled security-audit workflow.

## Runtime config

`config/default.toml` is the committed source of runtime tunables (cadences,
thresholds, endpoints, which compiled transport is active). Precedence: NVS
value → `config/default.toml` → built-in constant. Secrets (Wi-Fi
credentials, API key, client certificate and key) live only in NVS — never
in this file, which is why it can be committed. The TOML loader + NVS
override layer land with the config task; `firmware-core`'s `Default` impl
currently carries these exact values.
