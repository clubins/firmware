# Firmware research: current state, build verification, CI contract

> Read-only research of `lifelikemaik/clubins-hack` (branch `main` @ `312f956`), delivered 2026-10-30 by the Obvious research run. Method: every claim below was read from the repo this session; the ESP32-S3 build was actually executed in the research sandbox. This document is retained as the evidence base for the build spec.

**Load-bearing assumption verified up front: a sandbox CAN compile ESP32-S3 firmware.** Nix 2.35.4 was installed single-user, the CI-exact pinned toolchain (Rust fork 1.97.0.0 + GCC esp-15.2.0_20250920) was built via `nix-shell --pure`, and a real ESP32-S3 image was linked with esp-hal 1.2.2 in 24.5s. Recipe + gotchas below.

## 1. Current state

- `hardware-firmware/src/main.rs` is a **std host Hello World** (prints "Hello, world!"). No dependencies (Cargo.toml has none), no ESP code, no features. `cargo test`: 0 tests.
- The only sensor design is **decibel** (`hardware-firmware/implementation_notes/decibel.md`): SPK0641HT4H-1 PDM mic → I2S0 PDM@48kHz → A/C weighting → LAeq/LCeq + Fast extrema → overload detection → per-unit calibration. Unimplemented. `decibel.md` specifies esp-hal 1.2.2.
- **Implemented sensor types in firmware: none.**
- Server-side types a sensor may emit (evidence: `server-infra/ingest/measurements/measurements.go`): **decibel** (dba, dbc), **pax** (occupancy_count), **weather** (temperature, humidity, precipitation, rain_probability, wind_speed, pressure — currently virtual Go sensors), **status** (subsystem, result, message). Postgres `sensor_types` is a **set** — one device may do several (Skatehalle "db-counter" does decibel + pax).
- Wire contract today: HTTP JSON POST to `ingest.clubins.de/v1/sensors/<type>` — common fields `sensor_type`, `event_id` (UUIDv7), `time` (ISO 8601), `venue_id` (placeholder, server resolves from Postgres). Planned: MQTT v2 protobuf (`api/mqtt/topics.md`, `api/mqtt/v2/{header,status,decibel}.proto`): per-type topics, boot_id + per-topic seq dedup, QoS 1, retained status; "a new measurement type gets its own topic and its own file under v2/sensor/". Identity for v2: mTLS client certs from `server-infra/pki/`, CN = sensor UUID (rmqtt :8884 ready but unused; CRL limitation noted in TODO.md).

## 2. Build verification in sandbox (evidence)

Works:
- `cd hardware-firmware && nix-shell --pure --run 'cargo clippy --locked -- -D warnings && cargo build --locked && cargo test --locked'` → all green, ~1.4s cached (first cold ~1s + deps fetch). `just --list` and `just build` also green (just v1.59.0 installed via cargo).
- ESP32-S3 probe (scratch crate outside repo, esp-hal =1.2.2): `nix-shell hardware-firmware/shell.nix --pure --run 'cargo build --release --target xtensa-esp32s3-none-elf -Zbuild-std=core'` with `.cargo/config.toml` = `linker="rust-lld"` + `rustflags=["-C","link-arg=-Tlinkall.x"]` + `build-std=["core"]` → `Finished release in 24.52s`, artifact `ELF 32-bit LSB executable, Tensilica Xtensa … statically linked`, **128,496 bytes**.

Fails (reproduced gotchas any ESP target build must respect):
- GNU ld (GCC driver): `cross-endian linking for …/symbols.o not supported` — do NOT link Xtensa with the GCC driver; use rust-lld (this is esp-hal's own CI recipe).
- rust-lld without `-Tlinkall.x`: `relocation R_XTENSA_SLOT0_OP out of range … references section '.literal.__atoi'` — esp-hal linker scripts are mandatory.
- rustc stable 1.99.1 breaks esp-hal ≤1.2.2 (global_asm! — README's stated reason for the 1.97.0.0 fork pin); fork ships no rust-std for the target so `-Zbuild-std=core` is mandatory.
- Network: crates.io, GitHub releases, static.rust-lang.org, cache.nixos.org all reachable. nix install: `sudo mkdir /nix && sudo chown user:user /nix` then `--no-daemon` installer; CI-exact shell cold-builds in ~1m40s. Repo stayed clean throughout (`git status` empty; only target/ written).

## 3. CI contract (`.github/workflows/hardware-firmware.yml`)

Self-hosted NixOS runner, 30-min timeout, skips fork-PR builds. Runs everything inside `nix-shell --pure`: `just lint json-diagnostic-rendered-ansi` and `just build json-…` piped through `.github/scripts/rustc-annotations.jq` → ::error/::warning annotations (max 10+10 per step); `just test`. Then nix garbage-collect + upload the firmware shell to Attic. Separate scheduled security workflow (Go/Rust/Dart/Python scanners; firmware `just audit` uses a separate security shell) and a separate fmt workflow (`just fmt-check` via treefmt). Must stay green: lint (-D warnings), build, test, fmt-check, security scans.

## 4. PR conventions

Conventional Commits (feat:/fix:/docs:/refactor:/test:), concise bodies, **signed commits required** (AGENTS.md; note: sandbox git has no signing key configured yet — needs setup before PRs). treefmt is central (rustfmt edition 2024, max_width 90; pre-commit hook). Cargo.lock committed, all recipes use --locked. Note from README lines ~49–52: the planned multi-crate multi-chip workspace needs per-target `target.<triple>.rustflags`, which makes Cargo ignore `build.rustflags` and silently drops the `--remap-path-prefix` diagnostics remap — the jq filter exists for that reason; moving to a repo-root Cargo workspace would let a plain problem matcher replace it. That README section is the only trace of the workspace plan in the repo.

## 5. Constraints shaping the feature-flag framework

- **no_std for the device target**: the real firmware moves from the std placeholder to esp-hal (no_std) — serde/TOML crates must be no_std+alloc compatible; runtime config realistically lands in flash-backed NVS (esp-storage) for device identity (sensor UUID, per-unit calibration from decibel.md), while capability flags are natural **cargo features** (compile-time, CI matrix over combos) — 'config file' should be split into build-time feature flags + runtime NVS config.
- **Binary size**: esp-hal minimal image is ~128 KB; ESP32-S3 typically has 8 MB flash — headroom is fine, but TLS (esp-mbedtls for mTLS MQTT or HTTPS) and Wi-Fi stacks are the big adders; feature-gating them matters.
- **Linking**: any new target build must use rust-lld + -Tlinkall.x + -Zbuild-std=core (gotchas above); multi-crate workspace will need the per-target rustflags config the README anticipates.
- **CI compatibility**: feature-flag default set must keep host checks green (placeholder-style std build may stay for fast CI; ESP target build should be added to CI now that the recipe is proven).
- **Server contract**: every new sensor type needs (a) its own topic + proto file under api/mqtt/v2/ AND/OR an ingest endpoint/ClickHouse table, (b) Postgres sensor_types entry — firmware features should mirror this registry.
- **Identity**: v2 MQTT uses mTLS CN=sensor UUID; today's HTTP uses per-sensor API key — transport should stay feature-selectable.
