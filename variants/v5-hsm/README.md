# v5-hsm — hierarchical state-machine (event-driven) firmware

Variant E of the Clubins firmware architecture build: the classic QP-style
event-driven embedded design. Every subsystem is a hierarchical state machine
(nested states, entry/exit actions, deferred events, static transition
tables) running over one central event dispatcher. There are no preemptive
tasks and no superloop — deep sleep is a top-level system state, and the
dispatch loop is the whole runtime.

See `docs/spec/` (repo root) for the shared 15-flag build spec this variant
implements, and `docs/plan/build-dag.md` for where this scaffold sits in the
delivery plan.

## Architecture map

```
                      ISR / timer callbacks (downstream PRs)
                                        │  post
                                        ▼
              ┌──────────────────────────────────────────────┐
              │                dispatcher.rs                 │
              │  priority-ordered machines · FIFO per queue  │
              │  run-to-completion steps · bounded queue     │
              │  drop counters · post-dispatch defer recall  │
              └──────────────────────┬───────────────────────┘
                                     │ deliver (subscription-filtered)
        ┌────────────────┬───────────┼───────────────┬──────────────┐
        ▼                ▼           ▼               ▼              ▼
   SystemMachine   sensor stubs  transport stubs  ops stubs   …one per
   (system.rs)     (sensors/*)   (transports/*)   (ops/*)      compiled flag
        │
        ▼  fsm.rs — the core primitive (one HSM per machine)
┌───────────────────────────────────────────────────────────────────────┐
│  Hierarchical FSM: static StateDef table (name, parent, entry, exit,  │
│  handler, transitions) · validated at construction · LCA transitions  │
│  (exit leaf→LCA, action, enter LCA→leaf) · initial (INIT) transitions │
│  descend without re-entry · deferred-event queue recalled when a      │
│  machine re-enables · observable stats (events, transitions, drops)   │
└───────────────────────────────────────────────────────────────────────┘

Event flow:  post ──► queue (FIFO) ──► dispatch step ──► HSM run-to-completion
                                      └─ posts during delivery join the tail
```

### System machine (the lifecycle demonstration)

`system.rs` exercises the framework end to end with the device lifecycle as
a six-state hierarchy:

```
sys (root: fault latch, link state from any depth)
├── boot      ── SIG_BOOT_DONE ──► running      (bring-up completes as an event)
├── running   (initial ──► sampling)
│   ├── sampling   ── SIG_MEAS_READY ──► publishing
│   └── publishing ── SIG_PUBLISH_OK  ──► sampling
│                 ── SIG_PUBLISH_FAIL ──► defer (recalled when the link returns)
├── sleeping  (top-level: SIG_SLEEP exits the whole running composite)
└── fault     (latched from any depth via the root handler)
```

The transitions that matter architecturally:

- **Deferred events** — a publish failure while offline defers `SIG_PUBLISH_OK`
  follow-ups; the dispatcher's post-dispatch recall drains them on
  `SIG_WIFI_UP` (the deferred event re-runs the failed publish).
- **Top-level transition** — `SIG_SLEEP` is handled by `running` and targets
  `sleeping`: the LCA is the root, so the exit chain is sampling → running,
  innermost first. Deep sleep is a top-level system state, not a subsystem.
- **Fault latch** — `SIG_FAULT` is handled at the root, so it transitions out
  of any depth, and the fault state swallows further traffic.

## Flag matrix

Capability flags are cargo features — the compile-time half of "a flag in a
config file". Every public leaf is independent: any combination must compile
(CI's `flag-matrix` job builds all-off, each leaf alone, and all-on). The
internal plumbing flags keep dependency graphs honest and are pulled in by
the leaves.

| Flag | What it compiles in | Depends on |
| --- | --- | --- |
| `sensor-decibel` | SPK0641HT4H-1 PDM mic stub (I2S0 @ 48 kHz DSP later) | — |
| `sensor-pax` | PIR occupancy front-end stub | `driver-gpio` |
| `sensor-bme280` | temperature + humidity + pressure stub | `driver-i2c` |
| `sensor-sht4x` | temperature + humidity stub | `driver-i2c` |
| `sensor-scd4x` | CO₂ stub | `driver-i2c` |
| `sensor-pms5003` | PM2.5 stub | `driver-uart` |
| `sensor-bh1750` | ambient light stub | `driver-i2c` |
| `transport-http` | HTTP-JSON ingest stub (working server path) | `net-wifi` |
| `transport-mqtt` | MQTT v2 protobuf stub per `api/mqtt` | `net-wifi`, `proto-v2` |
| `transport-mtls` | mTLS client-certs stub | `transport-mqtt` |
| `ota` | esp-ota update stub | — |
| `provisioning` | BLE Wi-Fi provisioning stub | `net-wifi` |
| `deep-sleep` | duty-cycled sampling stub | — |
| `logging-defmt` | defmt logging stub | — |
| `ops-status` | status heartbeat + boot lifecycle stub | — |

Internal plumbing (not part of the public matrix): `driver-i2c`,
`driver-uart`, `driver-gpio`, `net-wifi`, `proto-v2`.

Defaults: `sensor-decibel`, `transport-http`, `ops-status` — exactly what the
current ingest server can receive.

## Layout

```
variants/v5-hsm/
├── Cargo.toml               # the 15 public flags + 5 plumbing flags
├── build.rs                 # links esp-hal's Xtensa linker scripts on target
├── justfile                 # check · flag-matrix · build-esp · fmt · lint
├── shell.nix                # pinned nixpkgs + esp-toolchain + just + espflash
├── esp-toolchain.nix        # esp-rs fork 1.97.0.0 + xtensa GCC 15.2.0
├── rustfmt.toml             # repo convention: edition 2024, max_width 90
├── .cargo/config.toml       # xtensa target: rust-lld + -Tlinkall.x
└── src/
    ├── lib.rs               # alloc wiring + module gates
    ├── events.rs            # copyable signals/events; INIT reserved (0)
    ├── fsm.rs               # the core primitive: hierarchical FSM framework
    ├── dispatcher.rs        # central dispatcher + Machine trait
    ├── system.rs            # system machine + feature-gated composition root
    ├── stubkit.rs           # shared stub lifecycle machinery
    ├── main.rs              # thin Xtensa entry point (host stub for check)
    ├── sensors/             # one stub per sensor-* flag
    │   ├── decibel.rs  ├── pax.rs  ├── bme280.rs
    │   ├── sht4x.rs    ├── scd4x.rs ├── pms5003.rs
    │   └── bh1750.rs
    ├── transports/          # one stub per transport-* flag
    │   ├── http.rs  ├── mqtt.rs  └── mtls.rs
    └── ops/                 # one stub per ops flag
        ├── status.rs  ├── ota.rs  ├── provisioning.rs
        ├── deep_sleep.rs  └── logging.rs
```

## Building

Everything runs from this directory, inside the pinned shell — there is no
repo-root toolchain:

```sh
nix-shell --pure --run 'just check'        # fmt, clippy -D warnings, build, test
nix-shell --pure --run 'just flag-matrix'  # all-off, each leaf alone, all-on
nix-shell --pure --run 'just build-esp'    # ESP32-S3 release ELF (default flags)
nix-shell --pure --run 'just build-esp "--all-features"'
```

Host tests run on the host toolchain inside the same shell; the pure-logic
modules (`events`, `fsm`, `dispatcher`, `system`, `stubkit`, the stubs)
compile and test off-target, while `esp-hal`/`esp-alloc` are pulled in only
under `cfg(target_arch = "xtensa")`.

### Toolchain notes (deliberate choices)

- **No `rust-toolchain.toml`.** The esp-rs fork is not a rustup channel: with
  an override file present, cargo defers to rustup for a channel that can
  never be installed ("custom toolchain '1.97.0.0' is not installed"). The
  toolchain comes exclusively from `esp-toolchain.nix` via `nix-shell --pure`
  PATH — the same arrangement as the source monorepo's `hardware-firmware/`.
- **`-Zbuild-std=core,alloc`.** The fork ships no prebuilt rust-std for
  Xtensa; both core and alloc are rebuilt for the target (E0463 on
  `extern crate alloc` with core alone).
- **Heap on target.** esp-alloc 0.10 supplies the `#[global_allocator]`
  (`global-allocator` default feature); `main` registers a 64 KiB internal
  heap region before the dispatcher allocates.

## Verification story

- Host: 49 tests across events/fsm/dispatcher/system/sensors/transports/ops —
  transition semantics, hierarchy validation, deferred-event delivery and
  recall, dispatcher ordering and isolation, lifecycle composition.
- Flag matrix: all-off + each of the 15 leaves alone + all-on compile.
- Target: release ELF links for `xtensa-esp32s3-none-elf` under the default
  and all-feature sets.
- CI (`.github/workflows/ci-v5.yml`, GitHub-hosted `ubuntu-latest` + Nix):
  `check`, `flag-matrix`, and `xtensa` jobs run on every PR touching this
  variant.

Nothing here touches hardware yet: drivers, real transports, and NVS-backed
config arrive as downstream PRs against the same flags (the stubs define the
Machine seams they will fill).
