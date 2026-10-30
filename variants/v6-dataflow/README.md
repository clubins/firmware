# v6-dataflow — statically-allocated lock-free dataflow firmware

Variant F of the Clubins firmware architecture build: the system is a
statically-allocated graph of lock-free SPSC ring buffers. DMA engines
(I2S/PDM, UART) push bytes directly into rings; processing stages drain
rings with zero heap and no locks; transports are graph drains; the memory
budget is fixed at link time.

See `docs/spec/` (repo root) for the shared 15-flag build spec this variant
implements, and `docs/plan/build-dag.md` for where this scaffold sits in the
delivery plan.

## Architecture map

```
   I2S/PDM DMA ─┐                                   (downstream PRs: real
   UART DMA   ──┤→  SPSC rings  →  stages  →  SPSC rings  →  drains
   GPIO ISR   ──┘   (bytes)       (parse,        (measurements)  (transports,
                                   weight, fold)                 ops)
```

- **The universal primitive is the SPSC ring** (`ring.rs`): a
  power-of-two-capacity, lock-free, single-producer/single-consumer queue
  with free-running head/tail counters. The producer never blocks — when a
  ring is full the push is rejected and counted (DMA overrun semantics: the
  engine's private frame is reused). The consumer never blocks — when it
  falls behind it skips stale items to the newest (`skip`) and the lag is
  counted. No locks, no atomics RMW loops, no heap.
- **DMA-backed sources** (`dma.rs`): a simulated DMA engine hands fixed-size
  frames of bytes into a ring, all-or-nothing (frame atomicity), and counts
  overruns. I2S/PDM and UART engines are simulated on host with the real
  domain rates (48 kHz PDM, 115_200 Bd UART).
- **Graph** (`graph.rs`): the pump. One `step()` advances every compiled
  node once: DMA transfer → stage drain (bytes → measurements) → route into
  every compiled transport's input ring → drain. Nodes live inline in the
  graph struct — statically allocated, sized by `size_of`.
- **Fixed memory budget** (`budget.rs`): the whole graph is one `static`
  (via `cell::StaticCell`), so its footprint is exactly `size_of::<Graph>()`
  at link time; a const assertion fails the build if it exceeds
  `BUDGET_BYTES`.

## Flag matrix

Capability flags are cargo features — the compile-time half of "a flag in a
config file". Every public leaf is independent: any combination must compile
(CI's `flag-matrix` job builds all-off, each leaf alone, and all-on). The
internal plumbing flags keep dependency graphs honest and are pulled in by
the leaves.

| Flag | What it compiles in | Depends on |
| --- | --- | --- |
| `sensor-decibel` | SPK0641HT4H-1 PDM mic source (I2S0 @ 48 kHz DSP later) | — |
| `sensor-pax` | PIR occupancy front-end (GPIO ISR producer) | `driver-gpio` |
| `sensor-bme280` | temperature + humidity + pressure source | `driver-i2c` |
| `sensor-sht4x` | temperature + humidity source | `driver-i2c` |
| `sensor-scd4x` | CO₂ source (new type) | `driver-i2c` |
| `sensor-pms5003` | PM2.5 source (UART DMA front-end) | `driver-uart` |
| `sensor-bh1750` | ambient light source (new type) | `driver-i2c` |
| `transport-http` | HTTP-JSON ingest drain (working server path) | `net-wifi` |
| `transport-mqtt` | MQTT v2 protobuf drain per `api/mqtt` | `net-wifi`, `proto-v2` |
| `transport-mtls` | mTLS client-certs drain | `transport-mqtt` |
| `ota` | esp-ota update drain | — |
| `provisioning` | BLE Wi-Fi provisioning | `net-wifi` |
| `deep-sleep` | duty-cycled sampling quiesce | — |
| `logging-defmt` | defmt logging sink | — |
| `ops-status` | status heartbeat + boot lifecycle | — |

Internal plumbing (not part of the public matrix): `driver-i2c`,
`driver-uart`, `driver-gpio`, `net-wifi`, `proto-v2`.

Defaults: `sensor-decibel`, `transport-http`, `ops-status` — exactly what the
current ingest server can receive.

## Layout

```
variants/v6-dataflow/
├── Cargo.toml               # the 15 public flags + 5 plumbing flags
├── build.rs                 # links esp-hal's Xtensa linker scripts on target
├── justfile                 # check · flag-matrix · build-esp · fmt · lint
├── shell.nix                # pinned nixpkgs + esp-toolchain + just + espflash
├── esp-toolchain.nix        # esp-rs fork 1.97.0.0 + xtensa GCC 15.2.0
├── rustfmt.toml             # repo convention: edition 2024, max_width 90
├── .cargo/config.toml       # xtensa target: rust-lld + -Tlinkall.x
└── src/
    ├── lib.rs               # module gates (one module per public flag)
    ├── main.rs              # thin Xtensa entry point (host stub for check)
    ├── ring.rs              # the core primitive: lock-free SPSC ring
    ├── cell.rs              # StaticCell: one-time static init (no allocator)
    ├── dma.rs               # simulated DMA engines (I2S/PDM, UART)
    ├── measurement.rs       # the contract-typed measurement payload
    ├── nodekit.rs           # shared node machinery (rings, counters)
    ├── graph.rs             # the pump: DMA → stages → routing → drains
    ├── budget.rs            # link-time memory budget accounting
    ├── system.rs            # feature-gated composition root
    ├── sensors/             # one source node per sensor-* flag
    │   ├── decibel.rs  ├── pax.rs  ├── bme280.rs
    │   ├── sht4x.rs    ├── scd4x.rs ├── pms5003.rs
    │   └── bh1750.rs
    ├── transports/          # one drain per transport-* flag
    │   ├── http.rs  ├── mqtt.rs  └── mtls.rs
    └── ops/                 # one node per ops flag
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
modules (`ring`, `cell`, `dma`, `measurement`, `nodekit`, `graph`, `budget`,
`system`, the nodes) compile and test off-target, while `esp-hal` is pulled
in only under `cfg(target_arch = "xtensa")`.

### Toolchain notes (deliberate choices)

- **No `rust-toolchain.toml`.** The esp-rs fork is not a rustup channel: with
  an override file present, cargo defers to rustup for a channel that can
  never be installed ("custom toolchain '1.97.0.0' is not installed"). The
  toolchain comes exclusively from `esp-toolchain.nix` via `nix-shell --pure`
  PATH — the same arrangement as the source monorepo's `hardware-firmware/`
  and the merged variants.
- **`-Zbuild-std=core` (no alloc).** The fork ships no prebuilt rust-std for
  Xtensa, so core is rebuilt for the target. V6 does not need `alloc`: the
  static graph allocates nothing, no global allocator is registered, and no
  `alloc` item is reachable — the stronger form of "zero heap after init".
  A downstream PR that needs a heap (Wi-Fi/TLS) adds esp-alloc behind its
  own flag and switches this to `core,alloc` (V5's recipe).
- **No heap on target.** `main.rs` registers no allocator; the graph lives
  in a `static` via `cell::StaticCell`, so its footprint lands in `.bss`/
  `.data` at link time — which is what makes the budget check honest.
