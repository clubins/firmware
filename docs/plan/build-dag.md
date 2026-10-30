# Build execution plan — 17-task DAG

> The approved build spec (`docs/spec/clubins-firmware-full-feature-flagged-build-spec.html`) decomposed into PR-sized subtasks. Each subtask = one PR with conventional commits, named verification, CI green before done. Dependencies encode ordering; independent tasks run in parallel waves.

Release strategy: multi-PR independent, every PR targets `main` of this repo. Squash merge.

## Wave 1 (foundation)

| # | Branch | Task | Complexity | Verified by |
|---|--------|------|------------|-------------|
| 1 | `feat/firmware-scaffold` | Scaffold esp-hal crate + flag framework | high | host `just check` + Xtensa release build (default / --all-features / --no-default-features); CI green |

Replaces the hello-world placeholder in `hardware-firmware/` with an esp-hal 1.2.2 ESP32-S3 no_std app, a host-testable lib layout, and all 15 capability features + support features declared with correct umbrella wiring. defmt logging transport wired here. Copies and adapts the pinned Nix toolchain (`esp-toolchain.nix`, `shell.nix`) and CI workflow from the source monorepo.

## Wave 2 (parallel, after scaffold)

| # | Branch | Task | Complexity | Verified by |
|---|--------|------|------------|-------------|
| 2 | `feat/config-nvs` | Config model: default.toml + NVS overrides | high | host tests: precedence (built-in < file < NVS), unknown-key tolerance, secret-exclusion |
| 3 | `feat/sensor-core` | Sensor core: measurement model + one-topic-per-type | standard | host tests: payload shapes, exact topic mapping for all six types |
| 4 | `feat/decibel-dsp` | Decibel DSP per decibel.md (pure, host-tested) | high | host tests: ±0.5 dB of reference vectors, overload at threshold, window extrema/Leq, calibration shift |
| 5 | `feat/driver-buses` | Driver buses: I2C, UART, GPIO front-ends | standard | host tests: PMS5003 frame parse/checksum, PIR debounce; Xtensa compile per feature |
| 8 | `feat/transport-core` | Transport core: Wi-Fi bring-up + clock + Transport trait | high | host tests: backoff schedule, monotonic fallback; Xtensa compile-check |

## Wave 3 (after wave 2)

| # | Branch | Task | Depends on | Complexity | Verified by |
|---|--------|------|------------|------------|-------------|
| 6 | `feat/drivers-i2c-uart` | BME280, SHT4x, SCD4x, PMS5003, BH1750 drivers | 3, 5 | standard | host conversion-math tests; each driver compiled individually + together on Xtensa |
| 7 | `feat/driver-decibel-pax` | PDM microphone + PIR occupancy drivers | 3, 4, 5 | standard | host event-assembly tests; Xtensa compile-check |
| 9 | `feat/transport-http` | HTTP transport: JSON encoder + ingest post | 8, 3 | standard | host GOLDEN tests: byte-exact JSON vs Go-struct fixtures |
| 10 | `feat/transport-mqtt` | MQTT transport: protobuf v2 + topic lifecycle | 8, 3 | high | host conformance tests vs `.proto` wire fixtures; seq/boot_id lifecycle |
| 12 | `feat/ops-status` | Ops status: heartbeat + boot lifecycle (default-on) | 8, 3 | standard | host tests: heartbeat assembly, lifecycle state machine |
| 13 | `feat/ota` | OTA updates behind ota flag | 8 | standard | host tests: verify/rollback state machine; Xtensa compile-check |
| 14 | `feat/provisioning` | BLE Wi-Fi provisioning behind provisioning flag | 8 | high | host tests: credential chunking, storage isolation; Xtensa compile-check |
| 15 | `feat/deep-sleep` | Deep-sleep duty cycling behind deep-sleep flag | 8, 3 | standard | host tests: scheduler math, seq continuity across simulated sleeps |
| 16 | `feat/defmt-logging` | defmt logging behind logging-defmt flag | 1 | low | host tests: level gating, secret redaction |

## Wave 4+

| # | Branch | Task | Depends on | Complexity | Verified by |
|---|--------|------|------------|------------|-------------|
| 11 | `feat/transport-mtls` | mTLS: client certs from NVS | 10 | standard | host tests: identity isolation; Xtensa compile-check |

## Final gate

| # | Branch | Task | Depends on | Verified by |
|---|--------|------|------------|-------------|
| 17 | `chore/flag-matrix-verify` | Flag-matrix integration verification + README | all above | full matrix run: `--no-default-features`, every leaf alone, Xtensa default/all-features/each transport; README feature-matrix table |

## Hard rules across all PRs

- Contract exactness: HTTP-JSON field names match the Go ingest structs (`sensor_id`, `apikey`, `time`, `measurement_id`, `dba`/`dbc`, `occupancy_count`, weather fields, status fields); MQTT matches `api/mqtt/topics.md` + `api/mqtt/v2/*.proto` (one topic per type; seq per topic from 0; stable `boot_id`; retained status on connect).
- Decibel DSP follows `implementation_notes/decibel.md` (A/C weighting, overload, windows, per-unit calibration).
- Secrets only ever from NVS at runtime; never in `config/default.toml`, never serialized, never logged.
- no_std discipline: pure logic host-tested; hardware glue compile-verified on the Xtensa target.
- Visual evidence is not applicable (backend/no_std firmware work).
