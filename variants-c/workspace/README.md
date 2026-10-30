# C2 — workspace-composed firmware (ESP-IDF, multi-component)

The C port of Rust V2's workspace architecture: an ESP-IDF multi-component
project with one component per concern — a shared core, one component per
sensor driver, the transports, the net plumbing, the ops capabilities —
composed by a slim `main` app. Components are pulled into the build by the
Kconfig capability selection (`CONFIG_CLUBINS_CAP_*`); a capability that is
not selected has no component in the link at all (link-time pruning), and a
flag wired to a missing component is a link error, never a null callback at
boot.

Everything here is a scaffold stage of the real firmware: the component
framework, the wiring, and the contracts are real; each capability's body
is a stub whose real implementation lands in its own task. A stub is an
empty `*_init`/`*_tick` pair with a header comment naming the task that
will replace it — never a `// TODO` in the dispatch path.

## ESP-IDF pin

**ESP-IDF v5.5.6** — the stable 5.x release, pinned across all `variants-c/`
variants. Recorded here and in the CI workflow (the same
`espressif/esp-idf-ci-action` version input).

## Architecture map

```
variants-c/workspace/
├── CMakeLists.txt            ESP-IDF project entry; registers the grouped
│                             components (EXTRA_COMPONENT_DIRS) explicitly
├── caps.cmake                flag → component mapping (shared verbatim by
│                             the host harness — one source of truth)
├── Kconfig.projbuild         15 public capability flags + 5 plumbing flags
├── sdkconfig.defaults        base device defaults (all-off capabilities)
├── sdkconfig.defaults.all-features
│                             all-15 anchor for the device CI job
├── components/
│   ├── clubins_core/         the shared core (V2's firmware-core): typed
│   │                         measurement model, sensor contract (sensor.h),
│   │                         fixed-capacity sensor registry (registry.h/.c),
│   │                         HTTP JSON ingest codec (http_json.h/.c) —
│   │                         pure platform-agnostic C, no IDF headers
│   ├── drivers/              one component per driver family (V2's drv_*
│   │                         crates): drv_decibel · drv_pax · drv_bme280 ·
│   │                         drv_sht4x · drv_scd4x · drv_pms5003 ·
│   │                         drv_bh1750 — each implements sensor.h's ops
│   ├── net/                  net_wifi (station bring-up) · proto_v2 (MQTT
│   │                         v2 protobuf encoders — nanopb when they land)
│   ├── transports/           transport_http (HTTPS JSON ingest) ·
│   │                         transport_mqtt (esp-mqtt v2 client; mTLS is a
│   │                         connection mode inside this component, V2
│   │                         parity — TRANSPORT_MTLS selects it in Kconfig)
│   └── ops/                  ops_ota · ops_provisioning · ops_sleep ·
│                             ops_status · logging_defmt
├── main/                     the composition root (V2's firmware bin):
│   ├── CMakeLists.txt        flag → REQUIRES mapping (composition rules)
│   ├── main.c                boot + FreeRTOS timer loop (device glue; the
│   │                         only IDF-headered file)
│   ├── app_compose.h/.c      registry_from_features port — one #if arm per
│   │                         capability, composing drivers + hooks
│   └── idf_component.yml     component-manager manifest
├── host/                     CTest + Unity harness over the same core
│   │                         sources (no ESP-IDF)
│   ├── test_http_json_codec.c    byte-exact encode/decode/escaping/truncation
│   ├── test_ingest_conformance.c fixture-driven contract conformance
│   ├── test_registry_compose.c   registry + composition under every flag
│   │                             combination
│   ├── fixtures/ingest/      the exact wire bytes (requests, responses,
│   │                         and server-400 rejections)
│   └── unity/                vendored Unity 2.x (from ESP-IDF's copy)
├── tools/
│   └── flag-matrix.sh        17-combination matrix gate (host + device)
├── .clang-format             LLVM base, 4-space indent, 100 columns
└── .gitignore                build/, sdkconfig, managed_components/
```

## Flag matrix (the port of V2's 15 features → components)

Every public leaf is a Kconfig symbol; plumbing flags are internal
(dependency carriers only). `select` mirrors V2's cargo feature
dependencies; the flag → component mapping lives in `caps.cmake` and
`main/CMakeLists.txt`.

| Kconfig leaf (CONFIG_CLUBINS_CAP_*) | Component | Selects | Wire type |
|---|---|---|---|
| `SENSOR_DECIBEL` | `drv_decibel` | — | `decibel` |
| `SENSOR_PAX` | `drv_pax` | `DRIVER_GPIO` | `pax` |
| `SENSOR_BME280` | `drv_bme280` | `DRIVER_I2C` | `weather` |
| `SENSOR_SHT4X` | `drv_sht4x` | `DRIVER_I2C` | `weather` |
| `SENSOR_SCD4X` | `drv_scd4x` | `DRIVER_I2C` | `co2` |
| `SENSOR_PMS5003` | `drv_pms5003` | `DRIVER_UART` | `pm25` |
| `SENSOR_BH1750` | `drv_bh1750` | `DRIVER_I2C` | `light` |
| `TRANSPORT_HTTP` | `transport_http` | `NET_WIFI` | — |
| `TRANSPORT_MQTT` | `transport_mqtt` | `NET_WIFI`, `PROTO_V2` | — |
| `TRANSPORT_MTLS` | (mode of `transport_mqtt`) | `TRANSPORT_MQTT` | — |
| `OTA` | `ops_ota` | — | — |
| `PROVISIONING` | `ops_provisioning` | `NET_WIFI` | — |
| `DEEP_SLEEP` | `ops_sleep` | — | — |
| `LOGGING_DEFMT` | `logging_defmt` | — | — |
| `OPS_STATUS` | `ops_status` | — | — |

Plumbing flags (`DRIVER_I2C`, `DRIVER_UART`, `DRIVER_GPIO`, `NET_WIFI`,
`PROTO_V2`) are hidden Kconfig symbols: selected by the leaves that need
them, never set directly.

Defaults are **all-off** — the C mirror of V2's `default = []`: a bare
build is the framework itself. The all-15 anchor lives in
`sdkconfig.defaults.all-features` and is what the device CI job builds
second.

New ingest types (`co2`, `pm25`, `light`) are emitted per the
one-topic-per-type rule; no server receiver exists yet — the gap is
tracked for the backend, not the firmware (spec §Decisions).

## How the composition works

`main/app_compose.c` is the port of V2's `registry_from_features`: every
capability arm is `#if`-guarded, so a build's composition is exactly the
set of components the Kconfig selection required. Sensors register into the
fixed-capacity registry (`clubins_registry_*`, no heap — V2 boxes its
drivers; the C port keeps caller-owned static instances); transports and
ops capabilities join the hook table (`init`/`tick` pairs). `main/main.c`
initializes everything once, then drives `clubins_app_tick()` from a 1 s
FreeRTOS software timer — sample every composed sensor, then tick every
composed hook (the periodic shape this architecture keeps for the real
loop: sample → window → encode → publish).

The component dependency graph mirrors V2's crate graph exactly: every
driver REQUIRES `clubins_core`; `transport_mqtt` REQUIRES `clubins_core`
and `proto_v2`; `ops_status` REQUIRES `clubins_core`; the rest stand alone
until their consumers arrive. Standalone components that nothing requires
are link-pruned by CMake automatically.

Config precedence is planned as in V2: NVS value → committed defaults →
built-in constant, with secrets (Wi-Fi credentials, API key, client cert)
only ever in NVS. The committed defaults land with the config task.

## Ingest contract (HTTP)

`components/clubins_core/http_json.c` is the source-of-truth check for
what a C2 device puts on the wire. Request bodies are compact JSON in the
Go structs' declaration order (credentials, then `measurement_id`/
`status_id`, then the payload), verified against
`server-infra/ingest/service` in the backend repo. Numbers are scaled
fixed-point integers — dB and temperature in tenths, everything else whole
— so the device never touches floating point. The decoder mirrors the
server's `encoding/json` behavior with one deliberate tolerance: it skips
unknown keys without fully validating their JSON shape (the server rejects
any malformed JSON anywhere in the body); conforming senders only ever
decode well-formed bodies.

## Verification (this scaffold)

| Gate | Command | What proves |
|---|---|---|
| Host CTest | `tools/flag-matrix.sh` (host mode) | registry API, composition, and codec tests — under all 17 combinations |
| Ingest conformance | `ctest --test-dir build/matrix/host-all-on -R ingest_conformance` | every HTTP body decodes and re-encodes byte-exact against the wire fixtures; server-400 shapes rejected |
| Flag matrix (host) | `tools/flag-matrix.sh` | all-off, each leaf alone (+ its Kconfig selects), all-on |
| Device build (CI) | `tools/flag-matrix.sh --device` | 17 × `idf.py build` (pinned v5.5.6) — local/beefed-up runner; CI anchors default + all-features builds on hosted runners |
