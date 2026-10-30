# clubins/firmware

Clubins venue-sensor firmware for the ESP32-S3 (Rust, esp-hal 1.2.2, no_std).

Every capability — sensors, transports, operations — sits behind a Cargo feature flag so each device image only contains what it needs. See [docs/spec](docs/spec/) for the full specification.

## Status

- **Spec:** approved — full-feature flagged build covering 15 capability flags, seven sensor driver families, HTTP + MQTT v2 transports, config/NVS, mTLS, OTA, BLE provisioning, deep sleep, defmt diagnostics, and status reporting.
- **Implementation:** in progress. The plan is a 17-PR dependency DAG ([docs/plan/build-dag.md](docs/plan/build-dag.md)); this README will list shipped features as PRs merge.

## Documentation

| Document | Contents |
|---|---|
| [docs/spec/clubins-firmware-full-feature-flagged-build-spec.html](docs/spec/clubins-firmware-full-feature-flagged-build-spec.html) | Approved build spec: problem frame, current state, architecture, feature-flag matrix, sensor expansion, config model, verification criteria, decisions and risks |
| [docs/research/firmware-internals-build-and-ci.md](docs/research/firmware-internals-build-and-ci.md) | Evidence base: repo state at research time, proven ESP32-S3 build recipe (rust-lld + linker scripts + `-Zbuild-std=core`), CI contract, PR conventions, flag-framework constraints |
| [docs/research/integration-surface-data-contracts.md](docs/research/integration-surface-data-contracts.md) | Backend contracts the firmware must fulfill: HTTP JSON ingest (exact Go struct fields), MQTT v2 protobuf design, hardware inventory, dashboard needs |
| [docs/plan/build-dag.md](docs/plan/build-dag.md) | Execution plan: 17 PR-sized tasks, dependency waves, complexity, and per-task verification |

## Firmware (once scaffolded)

```
hardware-firmware/
├── src/            # no_std esp-hal app + host-testable pure logic
├── config/         # committed default.toml (tunables only — secrets live in NVS)
├── esp-toolchain.nix / shell.nix / justfile
└── README.md       # build instructions + feature matrix
```

Build and check (host): `nix-shell --pure --run 'just check'` (fmt, clippy -D warnings, build, tests).
ESP32-S3 target: `nix-shell --pure` then the release recipe in `hardware-firmware/README.md` (`rust-lld`, esp-hal linker scripts, `-Zbuild-std=core`).

## Conventions

- Conventional Commits (`feat:`, `fix:`, `test:`, `chore:`, `docs:`); treefmt with rustfmt edition 2024.
- One capability = one Cargo feature; sensor drivers emit typed measurements, one topic per measurement type.
