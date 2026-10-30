# V7 — dual-core partitioned firmware (scaffold)

Strict core partitioning on the ESP32-S3: **core 1 (APP CPU)** owns sensing +
DSP on a deterministic real-time loop; **core 0 (PRO CPU)** owns Wi-Fi, the
ingest transports, and ops. The cores never share state — every crossing goes
through a lock-free boundary ([`src/boundary.rs`]): a bounded SPSC measurement
queue (core 1 → core 0), a tiny SPSC ack queue (core 1 → core 0), and a
control queue (core 0 → core 1) whose producer-side mailbox discipline
([`src/mailbox.rs`]) keeps at most one command outstanding. A network-side
fault can stall publishing but never sampling: at worst the measurement queue
fills and core 1 counts drops.

## Architecture map

```text
        core 0 (PRO CPU — latency-tolerant)                core 1 (APP CPU — real-time)
┌──────────────────────────────────────────┐      ┌──────────────────────────────────────────┐
│ core0::transports  http | mqtt | mtls    │      │ core1::sensors                           │
│ core0::ops         ota | provisioning |  │      │   decibel | pax | bme280 | sht4x |       │
│                    deep-sleep | logging  │      │   scd4x | pms5003 | bh1750               │
│                    | status              │      │                                          │
│         │ drain measurements             │      │   │ poll drivers, apply control          │
│         ▼                                │      │   ▼                                      │
│    Core0System ── Core0Link ◀══ SPSC ══▶ Core1Link ── Core1System                     │
│         │ post control (latest wins)     │      │        │ send acks (SPSC)               │
└─────────┼────────────────────────────────┘      └────────┼───────────────────────────────┘
          └──────────────── control mailbox ◀──────────────┴─ (acks queue ◀) ────────────
```

The queues are statically allocated and split exactly once at boot
(`boundary::leak_split`) into a `Core1Link` and a `Core0Link`, one moved to
each core — the typed links are what make the split-once discipline
inviolable. The queue halves come from `heapless::spsc` (atomic cursors,
soundly `Send` when the payload is `Send`); the crate itself carries
`#![deny(unsafe_code)]` with exactly one allow: the app-core stack handoff
in `cores::boot`, which esp-hal's `start_app_core` signature requires.
`cores::Systems` runs the same runtimes and boundary on the host,
stepping the two "cores" separately — that is what the host test suite
exercises.

## Flag matrix

15 public leaves (spec §Feature Matrix); every combination must compile
(`just flag-matrix` builds all-off, each leaf alone, all-on).

| Flag             | Partition | What it compiles in                             | Depends on             |
| ---------------- | --------- | ----------------------------------------------- | ---------------------- |
| `sensor-decibel` | core 1    | PDM mic stub + DSP pipeline slot (dba, dbc)     | —                      |
| `sensor-pax`     | core 1    | GPIO PIR occupancy front-end (occupancy_count)  | `driver-gpio`          |
| `sensor-bme280`  | core 1    | Temp + humidity + pressure → weather            | `driver-i2c`           |
| `sensor-sht4x`   | core 1    | Temp + humidity → weather                       | `driver-i2c`           |
| `sensor-scd4x`   | core 1    | CO₂ → co2 (new type)                            | `driver-i2c`           |
| `sensor-pms5003` | core 1    | PM2.5 → pm25 (new type)                         | `driver-uart`          |
| `sensor-bh1750`  | core 1    | Ambient light → light (new type)                | `driver-i2c`           |
| `transport-http` | core 0    | Wi-Fi + HTTPS JSON ingest (working server path) | `net-wifi`             |
| `transport-mqtt` | core 0    | MQTT v2 protobuf client per `api/mqtt`          | `net-wifi`, `proto-v2` |
| `transport-mtls` | core 0    | mTLS client certs from NVS (CN = sensor UUID)   | `transport-mqtt`       |
| `ota`            | core 0    | esp-ota update handling                         | —                      |
| `provisioning`   | core 0    | BLE Wi-Fi credential provisioning               | `net-wifi`             |
| `deep-sleep`     | core 0    | Duty cycling; parks core 1 via the mailbox      | —                      |
| `logging-defmt`  | core 0    | defmt logging transport                         | —                      |
| `ops-status`     | core 0    | Status heartbeat + boot lifecycle               | —                      |

Plumbing flags (`driver-i2c`, `driver-uart`, `driver-gpio`, `net-wifi`,
`proto-v2`) are internal dependency-graph edges, not public capability
switches.

## Build

```sh
nix-shell --pure --run 'just check'                 # fmt, clippy -D warnings, build, tests
nix-shell --pure --run 'just flag-matrix'           # all-off / each leaf alone / all-on
nix-shell --pure --run 'just build-esp "--all-features"'  # ESP32-S3 release ELF
```

## Dual-core launch

The pinned esp-hal 1.2.2 exposes the APP-core launcher behind its `unstable`
feature: `esp_hal::system::{CpuControl, Stack}` with
`start_app_core(&mut Stack, closure)` (`src/system/multi_core.rs` in the
crate source). `#[esp_hal::main]` runs on the PRO CPU, so the sensing
partition starts on core 1 via that call. No other esp-hal version is pulled
in, and nothing else in the crate is unstable-gated.

## Design decisions

- **Acked control mailbox.** Control commands are rare and 1:1-acked; a
  stale `ParkSampling` must not queue behind or after a `ResumeSampling`.
  The channel is a 2-slot SPSC queue; the discipline
  (`mailbox::ControlMailbox`, producer-side only) keeps at most one
  command outstanding — a new intent coalesces at the producer until the
  ack releases the slot (`post_control` returns `false`, caller retries
  after the next ack drain).
- **Usable-capacity constants.** heapless's SPSC reserves one slot to
  distinguish full from empty (a `Queue<T, N>` holds N−1, per its
  docs); every queue is therefore declared `{ CAP + 1 }` internally so
  the public `*_Q_CAP` constants mean usable capacity — 64 measurements
  in flight, not 63.
- **Zero steady-state allocation.** Queue payloads are `Copy` enums; the
  boundary is heap-pinned once at boot (`leak_split`) and never touched by the
  allocator again. Core 1 has no allocator traffic at all.
- **Static dispatch on the real-time core.** Sensor drivers are named fields
  of `Core1System`, polled in fixed order — no vtables, no registry scan;
  worst-case tick timing stays inspectable.
- **PIR honesty.** No PIR part is documented anywhere; the pax stub reports
  motion detections, not a head count (spec §Sensor Expansion).
- **New ingest types.** co2, pm25, light are emitted per the
  one-topic-per-type rule; server receivers for them are a separate backend
  change (known gap in the build spec's Decisions section).

## Status

Scaffold: all 15 flags gate stub modules; the cross-core boundary
(atomic SPSC queues via heapless, acked control mailbox, partition traits,
per-core runtimes, host mirror) is implemented and host-tested (26 tests,
48 with all features); the ESP32-S3 release ELF links for default and
all-features on the pinned fork toolchain. Drivers, transports, config, and
ops bring-up land in downstream PRs (docs/plan/build-dag.md).
