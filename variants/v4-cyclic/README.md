# Clubins V4 firmware — time-triggered cyclic executive

ESP32-S3 sensor firmware built around a **deterministic tick scheduler**: a
hardware timer interrupt advances a fixed phase table, ISRs defer work into
lock-free queues that their phase consumes, and a software timer wheel handles
cadences slower than the tick grid. No superloop, no async executor — bounded
jitter and phase ordering are first-class, tested guarantees.

## Architecture

```text
 ISR context (hardware timer, GPIO edges, UART RX)
   │  push events into SpscRing — never run work inline
   ▼
 Executive::run_batch ─── PhaseTable (fixed order, periodic slots)
   │   ├─ sensor phase     Registry::sample → Sample
   │   ├─ processing phase DSP / aggregation (decibel windows)
   │   ├─ comms phase      Transport::publish / poll
   │   └─ ops phase        status heartbeats, watchdog service
   ▼
 TimerWheel ── slow cadences (report intervals, heartbeats) as timers
 JitterTracker ── per-slot last/max/mean lateness, surfaced in BatchReport
```

- `exec/` — the scheduler primitive: `PhaseTable` (insertion order is
  execution order; a phase runs at most once per tick on `tick % period ==
  offset`), `SpscRing` (lock-free ISR → phase handoff; overflow is counted
  and the value handed back), `TimerWheel` (fixed slots, periodic re-arm
  after the drain), `JitterTracker` (the bounded-jitter ledger), and the
  `Executive` (bounded catch-up: a stalled foreground services up to
  `MAX_CATCHUP` slots per batch in table order, reporting lateness and any
  dropped ticks honestly).
- `sensor.rs` / `registry.rs` — the driver contract and the static,
  feature-gated registry: compiled drivers × runtime enables; a disabled
  driver is absent, never polled.
- `sensors/` — seven drivers, each behind its own flag. Stubs document
  exactly which follow-up task lands the hardware work.
- `dsp.rs` — decibel pipeline seam (types + chain state) per
  `implementation_notes/decibel.md`; the filter math lands with the decibel
  tasks and is host-tested against synthetic sines.
- `transport/` — HTTP JSON (working server path) and MQTT v2 protobuf
  (designed contract), independent flags, runtime-selected by config.
- `measurement.rs` — wire model, field-exact with the Go ingest structs.
- `net.rs`, `ops/` — Wi-Fi/clock seams and status/OTA/provisioning/sleep/
  logging capabilities.

## Flag matrix

Every capability is a leaf cargo feature with no hidden coupling; **any
combination must compile** — CI builds all-off, each flag alone, and all-on
(`just flag-matrix`). Internal plumbing flags (`driver-i2c`, `driver-uart`,
`driver-gpio`, `net-wifi`, `proto-v2`) keep dependency graphs honest and are
never built alone. Defaults match what the ingest server receives today.

| Flag | What it compiles in | Depends on |
| --- | --- | --- |
| `sensor-decibel` | PDM mic front-end + DSP pipeline (per `decibel.md`); emits `decibel` | — |
| `sensor-pax` | GPIO PIR occupancy front-end; emits `pax` | `driver-gpio` |
| `sensor-bme280` | Temperature, humidity, pressure → `weather` | `driver-i2c` |
| `sensor-sht4x` | Temperature, humidity → `weather` | `driver-i2c` |
| `sensor-scd4x` | CO₂ → `co2` (new type) | `driver-i2c` |
| `sensor-pms5003` | PM2.5 → `pm25` (new type) | `driver-uart` |
| `sensor-bh1750` | Ambient light → `light` (new type) | `driver-i2c` |
| `transport-http` | Wi-Fi + HTTPS JSON ingest (working server path) | `net-wifi` |
| `transport-mqtt` | MQTT v2 protobuf client per `api/mqtt` | `net-wifi`, `proto-v2` |
| `transport-mtls` | mTLS client certs from NVS (CN = sensor UUID) | `transport-mqtt` |
| `ota` | esp-ota update handling | — |
| `provisioning` | BLE Wi-Fi credential provisioning | `net-wifi` |
| `deep-sleep` | Duty-cycled sampling between sleeps | — |
| `logging-defmt` | defmt logging transport | — |
| `ops-status` | Status heartbeat + boot lifecycle (on by default) | — |

Cargo features are the compile-time half of the flag framework; runtime
tunables (cadences, thresholds, endpoints, which compiled transport is
active) come from `config/default.toml` with NVS overrides in a later task.
Secrets live only in NVS, never in committed config.

## Build and check

Everything runs inside the pinned Nix shell (esp-rs Rust fork 1.97.0.0 +
Xtensa GCC 15.2.0; see `esp-toolchain.nix`, `shell.nix`):

```sh
nix-shell --pure --run 'just check'        # fmt + clippy -D warnings + build + tests
nix-shell --pure --run 'just flag-matrix'  # all-off, each flag alone, all-on
nix-shell --pure --run 'just build-target' # ESP32-S3 release ELF (default features)
nix-shell --pure --run 'just build-target --all-features'
```

Host builds compile the pure-logic personality with zero external
dependencies; `main.rs` on host is an empty shell — the library is the tested
surface. Target builds link with `rust-lld -Tlinkall.x -Zbuild-std=core` (see
`.cargo/config.toml` for why).

### Why there is no rust-toolchain.toml

The Nix shell is the single toolchain pin. We deliberately do not commit a
`rust-toolchain.toml`: cargo reads toolchain override files natively, and an
override naming the espup fork channel (`channel = "esp"`) is unresolvable
unless that channel was rustup-linked by `espup` — on hosted CI it fails even
inside `nix-shell --pure` with `error: custom toolchain 'esp' ... is not
installed`. The pinned fork comes from `esp-toolchain.nix`; installing it
outside Nix is `espup`'s job, documented in the comment header there.

## Status

Scaffold: the scheduler primitive is implemented and host-tested (phase
ordering, ISR queue handoff, timer-wheel cadence math, jitter accounting);
drivers, transports, and ops capabilities are flag-gated stubs whose contract
surfaces (wire field names, topic rules, sample shapes) are already fixed and
documented in-code. CI (`.github/workflows/ci-v4.yml`) runs the host checks,
the flag matrix, and the ESP32-S3 link on GitHub-hosted runners.
