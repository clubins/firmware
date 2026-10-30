# V8 — declarative pipeline engine (variants/v8-declarative/)

Variant H of the eight-way firmware architecture build: sensor pipelines are
**declared in data** — `config/pipelines.toml` (sources → stages → sinks with
cadences and thresholds) — and instantiated at boot by a small interpreter
over a statically-typed, feature-gated registry of stage kinds. Adding or
retuning a sensor is a TOML edit, not a recompile. Cargo features gate which
kinds exist; runtime TOML gates which instances run.

Self-contained like every variant: all toolchain files live in this directory
and CI runs from here (`nix-shell --pure --run 'just check'`). The repo root
hosts no toolchain for V8 and this crate never touches the root crate, other
variants, or `docs/`.

## Architecture map

```
config/pipelines.toml ──include_str!──▶ decl.rs (std-free TOML parse)
                                             │ typed Pipelines
                                             ▼
                     registry.rs  ◀──────graph.rs ── validate:
              kind lookup by feature flag      kinds, params, refs,
              (cfg-gated arms per kind)        cycles, cadence multiples
                                             │ Vec<Instance> (topo order)
                                             ▼
                      engine.rs ── Engine::tick(now_ms)
                      cadence scheduler: due pipelines, in dependency order
                                             │
                                             ▼
                      main.rs (device) / cargo run (host prints schedule)
```

Modules:

- **`decl.rs`** — typed view over the pipelines document, parsed with the
  serde-free `toml` `parse` feature; `no_std` compatible.
- **`registry.rs`** — the statically-typed kind registry: source / stage /
  sink kinds with parameter validation. Each kind's arm is behind the cargo
  feature that owns it; a flag that is off removes the kind from every
  lookup — a config edit cannot smuggle in uncompiled code.
- **`graph.rs`** — boot-time validation: unique names, kind resolution,
  parameter typing (typos become named errors), `pipe` reference existence,
  acyclicity, and cadence multiples (`pipe` consumers must not tick faster
  than their producer). Produces instances in topological order.
- **`engine.rs`** — the interpreter: `Engine::instantiate` (validate →
  instances) and `Engine::tick` (cadence scheduler; slow consumers catch up
  on their original grid without bursting).
- **`ops/`** — flag-gated operations stubs (OTA state machine, BLE
  provisioning visibility, deep-sleep budget, defmt level filter).

Stage behaviours (I2S PDM capture, weighting filters, I2C/UART buses,
Wi-Fi, transports) are the downstream driver PRs; the scaffold proves the
graph mechanics, gating, and failure modes host-side, and that the Xtensa
personality links.

## Flag matrix

Same 15 public leaves as the spec §Feature Matrix; plumbing flags
(`driver-i2c`, `driver-uart`, `driver-gpio`, `net-wifi`, `proto-v2`) stay
internal. Defaults: `sensor-decibel`, `transport-http`, `ops-status`.

| Flag | What compiles in | V8 gate |
| --- | --- | --- |
| `sensor-decibel` | PDM mic front-end + DSP pipeline | `decibel` source, `a-weight`/`window-stats` stages |
| `sensor-pax` | GPIO PIR occupancy front-end | `pir` source, `pax-integrate` stage |
| `sensor-bme280` | Temp + humidity + pressure | `bme280` source |
| `sensor-sht4x` | Temp + humidity | `sht4x` source |
| `sensor-scd4x` | CO₂ | `scd4x` source |
| `sensor-pms5003` | PM2.5 particulates | `pms5003` source |
| `sensor-bh1750` | Ambient light | `bh1750` source |
| `transport-http` | Wi-Fi + HTTPS JSON ingest | `http-ingest` sink |
| `transport-mqtt` | MQTT v2 protobuf client | `mqtt-ingest` sink |
| `transport-mtls` | mTLS client certs from NVS | `mqtt-ingest` sink accepts `tls = false` |
| `ota` | esp-ota update handling | `ops::ota` |
| `provisioning` | BLE Wi-Fi credential provisioning | `ops::provisioning` |
| `deep-sleep` | Duty-cycled sampling | `ops::deep_sleep` |
| `logging-defmt` | defmt logging transport | `ops::logging` |
| `ops-status` | Status heartbeat + boot lifecycle | `clock` source, `status-heartbeat` sink |

Example pipelines under non-default flags (enable the flag, then add):

```toml
# needs sensor-pax
[[pipeline]]
name = "pax-motion"
source = { kind = "pir", cadence_ms = 2000 }
[[pipeline.stages]]
kind = "pax-integrate"
[pipeline.sink]
kind = "http-ingest"
type = "pax"

# needs transport-mqtt (+ transport-mtls for the TLS default)
[[pipeline]]
name = "status-mqtt"
source = { kind = "clock", cadence_ms = 60000 }
[pipeline.sink]
kind = "mqtt-ingest"
type = "status"
tls = false
```

## Working on this variant

```sh
cd variants/v8-declarative
nix-shell --pure            # pinned fork toolchain (esp-toolchain.nix)
just check                  # fmt + clippy -D warnings + build + tests
just flag-matrix            # all-off, each leaf alone, all-on (+ anchor tests)
just build-esp              # ESP32-S3 release ELF (default features)
just build-esp "--all-features"
just run                    # host: print the resolved schedule
```

`config/pipelines.toml` is compiled into the binary as boot defaults; the
committed document instantiates under the default feature set.
