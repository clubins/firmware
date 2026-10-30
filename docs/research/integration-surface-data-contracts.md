Research complete. All four investigation areas are covered with primary-source evidence. Deliverable follows.

---

# Findings: Firmware Integration Surface — `lifelikemaik/clubins-hack`

**Method note:** everything below was read this session from the repo at `cmp_4CXLzNaT` (`/home/user/work/clubins-hack`, branch `main` @ `312f956`). No files were edited, committed, or pushed; no PR was created (findings-only task, so the visual-evidence criterion has its backend-only exception — nothing visual was produced). Line numbers refer to current `HEAD`.

## Executive summary

Two protocols coexist, and the single most important spec decision they force is which one the new firmware speaks:

1. **HTTP JSON ingest** — the only path that currently reaches ClickHouse from a sensor. Four `POST` endpoints, flat JSON body, shared credentials block, per-sensor API key + type check, `202` + ClickHouse async insert. Documented in `server-infra/README.md` ("HTTP Ingest Service") and implemented in `server-infra/ingest/`.
2. **MQTT v2 protobuf** — fully designed in `api/mqtt/topics.md` + `api/mqtt/v2/*.proto`, broker (rmqtt, mTLS, port 8884) deployed, PKI provisioning scripts ready — but **nothing publishes to rmqtt and no Go bridge reads it** (`server-infra/README.md`, "rmqtt" section: "Nothing publishes to it or reads from it yet"). The firmware implementation notes say the sound chain is designed but "None of this is implemented yet" (`hardware-firmware/implementation_notes/decibel.md:7`).
3. Legacy **v1 MQTT JSON → Mosquitto → Vector → HTTP** exists only for three unreflashed Skatehalle ESP32s; it is explicitly a compatibility layer, not a target.

The firmware crate itself is a placeholder: `hardware-firmware/src/main.rs` is `fn main() { println!("Hello, world!"); }` with an empty `Cargo.toml` (`[dependencies]` section empty).

---

## (a) The data contract the firmware must fulfill

### A1. Transport #1 — HTTP JSON (the proven path)

**Public endpoint:** `https://ingest.clubins.de/v1/sensors/{decibel,pax,weather,status}` — DNS at `clubins.de.txt:16`; Caddy `handle_path /v1/*` strips the prefix and proxies to `127.0.0.1:8080` (`server-infra/modules/api/ingest.nix:17-27`). The Go service registers `POST /sensors/decibel|weather|pax|status` (`server-infra/ingest/service/service.go:71-74`). The bare `/v1/sensors` path 404s (README: it "was the decibel endpoint until weather arrived").

**Common credentials block** — embedded flat in every JSON body (`service/service.go:46-52`):

| Field | JSON key | Type | Notes |
|---|---|---|---|
| Sensor ID | `sensor_id` | string UUID | lookup key; must parse as UUID (`service.go:134-138`) |
| API key | `apikey` | string | compared in SQL, plaintext column (`sensors/sensors.go:57-61`) |
| Time | `time` | string | RFC3339(Nano); fallback `now()` if absent/invalid/outside 1900–2299 (`service.go:188-194`) |

**Per-endpoint payloads** (exact Go structs):

- **Decibel** (`service/decibel.go:13-18`): `measurement_id` (string), `dba` (number, required), `dbc` (number, required). Note: the *legacy MQTT firmware also sent* `dba_min`, `dba_max`, `dbc_min`, `window_ms` — **all four are silently dropped** by Vector and have no ClickHouse columns (`modules/legacy/ingest-vector.nix:152-173`; README: "Four decibel fields are dropped").
- **Pax** (`service/pax.go:12-16`): `measurement_id`, `occupancy_count` — JSON number, must be finite, whole, 0…2³²−1 (`pax.go:51-65`).
- **Weather** (`service/weather.go:12-21`): `measurement_id`, `temperature`, `humidity`, `precipitation`, `rain_probability`, `wind_speed`, `pressure` — all required, fixed validation order (`weather.go:32-42`). Units (column names deliberately unit-free): °C, %, mm, %, km/h, hPa (`measurements/measurements.go:28-29`; `migrations/clickhouse/20260821_02_weather_and_status.sql` comments).
- **Status** (`service/status.go:25-31`): `status_id`, `subsystem` (required, open vocabulary — `power`, `battery`, `connection` are the in-repo examples), `result` (closed set, 400 otherwise: `started|stopped|success|degraded|failed`, `status.go:18`), `message`.

**Auth & enrichment:** one query, `SELECT venue_id, location, sensor_types FROM sensors WHERE uuid = $1 AND api_key = $2 AND active` (`sensors/sensors.go:57-61`). All auth failures collapse to `401`; endpoint type must be a member of the sensor's `sensor_types` array or `403` (`service.go:151-155`); check runs after auth. **`venue_id`/`location` are never read from the body** — the server stamps them from PostgreSQL (`measurements/measurements.go:3-4`); unknown JSON keys are ignored (so extra firmware telemetry is accepted, `202`, not an error).

**Validation/delivery semantics:** body cap 1 MiB → `413` (`service.go:26`); wrong JSON type on a known key → `400`; non-finite or > float32 values → `400` (`service.go:165-173`); `measurement_id` absent/malformed → server generates UUIDv7 (`service.go:177-185`); success is `202 {"status":"accepted"}` meaning queued via ClickHouse async insert with `wait_for_async_insert=0` — accepted, not durable (`measurements/measurements.go:88-96`). **One JSON object per request — arrays are rejected** (README: "batch.max_events = 1 is load-bearing"). No physical-range checks (`"dba": 400` would be stored). Duplicate `measurement_id`s are not deduplicated.

**Cadence:** no batching, one POST per measurement. The only working reference client (virtual sensors) posts every **30 s** (`modules/services/virtual-sensors.nix:32,47`) with a 15 s HTTP timeout (`virtual_sensors/client/client.go:61`), generating a UUID `measurement_id` and RFC3339Nano UTC `time` per reading (`client.go:65-83`). Expected intervals are recorded nowhere server-side (README "Monitoring: not implemented" — the alerting blocker).

### A2. Transport #2 — MQTT v2 protobuf (designed, unbuilt)

From `api/mqtt/topics.md` and `api/mqtt/v2/{header,sensor/decibel,status}.proto`:

- **Topics:** `v2/sensors/<uuid>/data/<data_type>` (QoS 1, not retained) and `v2/sensors/<uuid>/status` (QoS 1, **retained**). Payload is exactly one protobuf message; the topic names the decoder. Only `decibel` exists so far; "A new measurement type gets its own topic and its own file under `v2/sensor/`" — the designed extension point for new sensors.
- **Identity:** the UUID lives in the topic only, and equals the MQTT username, taken from the **CN of the sensor's mTLS client certificate**; "it cannot be spoofed" (`topics.md`, "Identity"). No sensor UUID, venue, or key travels in the payload.
- **`Header`** (every message): `time_ms` (fixed64, epoch ms UTC; **0 = clock not synchronised**; for a measurement, the end of its window), `boot_id` (16 random bytes from the hardware RNG, generated once per boot), `seq` (uint32 per-boot, per-topic, from 0; gap = lost messages; `boot_id`+`seq` is the dedup key).
- **`measurement_id` derivation** (server-side, `topics.md`): UUIDv8 = `boot_id[0..12] ‖ be32(seq)`, version nibble 8, variant `10`.
- **`Decibel` message:** `meta {window_ms: uint32, overload: bool}`, `a {laeq, la_fmin, la_fmax}`, `c {lceq, lc_fmax}` — all dB SPL; Leq fields are energy averages; min/max are Fast-weighted (125 ms, IEC 61672-1) extremes running continuously across windows. **No LCpeak by design** — the mic clips ~120 dB SPL below club peaks (`decibel.proto:49-54`).
- **`Status` message:** `header`, `connect_time_ms`, `firmware_version` (string, "e.g. \"0.3.1\" or a git describe string"), `connection_state` enum `Unknown/Online/Offline/Died`; published once per connect.
- **Session lifecycle:** new `boot_id` + seq reset per boot; `Status` on every connect (reconnect without reboot = same `boot_id`; reboot = new).
- **Broker:** rmqtt on **8884, mTLS-only**, client cert from the private PKI in `server-infra/pki/`, `cert_cn_as_username = true` (`modules/api/rmqtt.nix:135-160`); retained messages persist to disk across broker restarts. Mosquitto (8883, TLS with Let's Encrypt, **no** client cert) carries only the legacy v1 sensors (`modules/api/mosquitto.nix:96-113`).
- **Cert provisioning flow** (`pki/README.md`): `request-mqtt-client-cert.sh <sensor-uuid>` on the device (key + CSR; CN = sensor UUID) → CSR to server → `sudo ./issue-mqtt-client-cert.sh` (rejects `O ≠ clubins`) → device gets `<cn>.pem` / `-chain.pem`. Revocation via `revoke-mqtt-client-cert.sh` + CRL — **but rmqtt 0.24.0 cannot load a CRL** (`pki/TODO.md`). A future cert-issuance **microservice** "for a client-side provisioning tool" is already planned (`pki/TODO.md:3-9`).

### A3. Storage the firmware feeds (quoted schemas)

ClickHouse (`server-infra/migrations/clickhouse/`), all `MergeTree`, `PARTITION BY toYYYYMM(time)`, `ORDER BY (venue_id, sensor_id, time, measurement_id)`; shared leading columns `measurement_id/status_id UUID, sensor_id UUID, venue_id UUID (from PostgreSQL), location LowCardinality(String) (from PostgreSQL), time DateTime64(3,'UTC'), ingest_time DateTime64(3,'UTC')`:

- `m_decibel`: `dba Float32, dbc Float32` (20260821_01)
- `m_pax`: `occupancy_count UInt32` (20260822_01)
- `m_weather`: 6 × Float32 (20260821_02)
- `m_status`: `subsystem, result LowCardinality(String), message String` (20260821_02)
- Views: `v_sensor_last_seen` + parameterised `v_pax_smoothed` / `v_decibel_smoothed` (Leq energy-domain averaging).

PostgreSQL owns identity: `venues(uuid, name, latitude, longitude)`, `sensors(uuid, venue_id, sensor_types TEXT[], location, api_key plaintext, active)` (`migrations/postgres/20260812_01_init.sql`, `20260821_01`, `20260821_02`, `20260822_01`).

---

## (b) Sensor / hardware inventory — everything mentioned in the repo

| Hardware / sensor | Where referenced (file:line) | Status |
|---|---|---|
| **ESP32-S3** (Xtensa LX7) MCU | `AGENTS.md:5`; `README.md:19,26`; `hardware-firmware/README.md:9,59`; `server-infra/README.md:44,68,293` | the only MCU target; multi-crate/multi-chip workspace is "planned" (`hardware-firmware/README.md:49`) |
| **SPK0641HT4H-1** PDM MEMS microphone (Knowles/Syntiant) | `hardware-firmware/implementation_notes/decibel.md:9-11` | designed, not implemented; I2S0 PDM→PCM 48 kHz (lines 55-80); usable ~35–115 dB(A), ~40–115 dB(C); no LCpeak (mic clips ~120 dB SPL, lines 41-47); per-unit calibration offset **stored in flash** (lines 141-151) |
| **Decibel sensor** (A/C-weighted sound level) | `api/mqtt/v2/sensor/decibel.proto`; `server-infra/ingest/service/decibel.go`; ClickHouse `m_decibel` | live on 2 Skatehalle devices via legacy MQTT; HTTP contract ready |
| **Pax / occupancy (people counter)** | `ingest/service/pax.go`; `m_pax`; device client ID `clubins-db-counter-2-outdoor` (`server-infra/README.md:834`) | live; one physical unit `34bf9ca4` emits **both** decibel and pax (`ingest/sensors/sensors.go:24`); dashboard applies a `× 0.6` over-count calibration (`20260823_01_pax_smoothed_view.sql`); pax-only unit `ba7d6078` "Main Floor, Wand" (`20260822_01_sensor_types_and_skatehalle.sql`) |
| **Weather / environmental** (temp, humidity, precipitation, rain prob., wind, pressure) | `ingest/service/weather.go`; `m_weather`; `modules/services/virtual-sensors.nix:27-49` | **virtual only** — Open-Meteo HTTP, every 30 s, for Munich, Berlin, and Skatehalle (the latter "since the hardware there measures neither" — line 41-43) |
| **Status heartbeat / self-report** | `ingest/service/status.go`; `m_status`; subsystem examples `power`, `battery` (`README.md:728`, `service/status_test.go:114`), `lifecycle/upstream/ingest` (virtual-sensors README table) | live |
| **Dropped simulator-only types: entry events, air quality, sales** | `server-infra/README.md` "The MQTT-era tables are gone"; Vector `retired_sensor_types` blackhole sink (`modules/legacy/ingest-vector.nix:212`) | tables dropped; routes blackholed — a signal that `airquality` was once anticipated |
| **NTP**: `ntp.clubins.de`, chrony with NTS | `clubins.de.txt:22`; `modules/system/ntp.nix` | server-side only; firmware clock discipline must use it (`time_ms = 0` protocol semantics) |
| **espflash** (USB flashing) | `hardware-firmware/shell.nix:8` | the only "device tooling" that exists |

**Explicitly absent everywhere** (ripgrep over `*.md,*.go,*.rs,*.nix,*.py,*.dart,*.proto,*.toml,*.yaml`): no PIR, camera, CO₂, CO, VOC, PM2.5, SHT/BME/BMP part numbers, BLE, Wi-Fi provisioning, or battery-hardware mentions. If the spec wants occupancy by PIR or CO₂ sensors, nothing in this repo anticipates them beyond the (dropped) `airquality` simulator type.

---

## (c) Dashboard requirements & onboarding/OTA expectations

**What the Flutter app renders** (`mobile-app/lib/`):
- **Decibel page** (default after login, `pages/dashboard_shell.dart:31,43`): sensors list (`DecibelSensor{sensorId, location, types}` from `GET /app/v0/venues/{venueId}/sensors`) and history (`DecibelReading{time, dba, dbc}` from `GET /app/v0/venues/{venueId}/history/decibel?from&to&points`) — `models/decibel_data.dart:1-56`, `services/clubins_api_service.dart:21-70`. Venue is **hardcoded** to the Skatehalle demo: `venueId = '9c3f1e42-…'` (`clubins_api_service.dart:18-19`). Ranges 6H/24H/7D/30D (`pages/decibel_page.dart:49-54`).
- **Weather page** — reads **Open-Meteo directly**, not Clubins-ingested data (`services/open_meteo_service.dart:42-50`); the app never displays `m_weather`. Only persistence in the app: `SharedPreferences` key `last_weather_location` (`pages/weather_page.dart:62-78`).
- **Occupancy/pax: not in the app at all** — it exists in ingest/ClickHouse/Grafana only. The Go API exposes **decibel history only** (`server-infra/api/internal/httpapi/server.go:34-35`); sensors endpoint returns `{sensorId, location, types}` (`api/internal/venue/models.go:8-12`).
- **Auth:** Dex OIDC (`login.clubins.de`), Bearer ID token to `api.clubins.de`, clients `clubins-dashboard-web`/`-android` (`mobile-app/lib/auth/auth_controller.dart:7-59`; `mobile-app/README.md` "Staff login"). It is a "UI soft lock: the API remains public" (README). A Keycloak plan (`docs/auth-keycloak-plan.md`) would harden this — customer identities in IdP, venue grants in Clubins PostgreSQL, sensor data unchanged.
- **New sensor types** would need in the API surface: a history endpoint per type (only decibel exists) and `types[]` entries from the `sensor_types` column; the app's `DecibelSensor.types` already generalizes.

**Onboarding / WiFi provisioning / OTA: none exists anywhere.** Greps for `ota`, `firmware.update`, `provision`, `wifi`, `bluetooth`, `BLE`, `deep.sleep` find only: iOS code-signing "provisioning" (`DEVELOPMENT.md:73-76`), PKI certificate provisioning (`pki/`), and the planned cert microservice (`pki/TODO.md:5-9`). The only firmware-identity surface is the v2 `Status.firmware_version` field. Device registration today is: manual `INSERT INTO sensors …` seeded inert + operational activation (`20260822_01` migration; virtual-sensors README), and device identity (UUID) is the admin's choice. The dead-man's-switch monitoring plan (README, "Monitoring") says the missing input is a **per-sensor expected interval** — "most naturally a column on `sensors`" — which a firmware cadence setting must eventually match.

---

## (d) Product-implied firmware features the code doesn't cover yet

Product: "Sound levels, occupancy, and environmental conditions for venue staff, live." (`README.md` tagline)

1. **Everything firmware-side is missing.** No measurement, no transport, no driver, no config: placeholder `main.rs`, empty `Cargo.toml`, and `DEVELOPMENT.md:93` warns checks "compile a host-side placeholder crate; passing them does not verify an ESP32-S3 image."
2. **Transport choice is the pivotal spec decision.** HTTP-JSON is the only path with a working server, auth, and storage. MQTT-v2 has better semantics (QoS 1 at-least-once, `boot_id`+`seq` dedup/gap detection, retained status, cert identity that "cannot be spoofed", richer decibel payload incl. min/max/overload/window) but **no ingest bridge exists** — spec must either mandate building one or treat HTTP as primary with v2 as migration target. The README architecture diagram literally lists "future HTTP firmware" as the third writer class.
3. **Environmental conditions have no hardware story.** `m_weather` is fed only by virtual sensors. New sensor types (temp/RH at minimum, possibly CO₂ given the dropped `airquality` type) need: a protobuf file under `api/mqtt/v2/sensor/` (v2 pattern) *and/or* a new `/v1/sensors/<type>` endpoint + `m_*` table + `sensor_types` row (README's "Adding another virtual sensor type" checklist, steps 1-6, is the template for any new type).
4. **Config-file + feature-flag model is greenfield.** No firmware config exists. Server-side facts the config must reconcile: identity (sensor UUID + apikey for HTTP, or cert CN for v2), endpoint/broker address, cadence (currently hardcoded 30 s in Nix; expected-interval column planned in PostgreSQL), `window_ms` (dropped in v1, first-class in v2), and per-unit calibration offset stored in flash (`decibel.md:141-151`). Flags must degrade gracefully: unknown JSON keys are ignored server-side (forward-compatible extra telemetry), but the status `result` vocabulary is closed and adding a value is a deploy.
5. **Clock sync matters.** `time_ms = 0` means "clock not synchronised"; a real `time_ms` requires NTP (the operator runs `ntp.clubins.de`, chrony/NTS) or RTC discipline; the HTTP path silently falls back to `now()`, the v2 path preserves the 0 marker.
6. **Status reporting conventions to adopt:** healthy = silent, failures to `m_status` (virtual-sensors convention), closed result set, open `subsystem` vocabulary; `firmware_version` string is the only version telemetry — a flag framework should report enabled-feature sets through `message`.
7. **Sound measurement is fully specified and waiting.** `decibel.md` is a build-ready DSP spec (PDM 48 kHz capture, DC blocker, mic correction, 3+2 biquad weighting, Leq accumulation f32-block/f64-total, Fast detector, overload tracking, calibration procedure) explicitly scoped to produce the v2 `Decibel` message. The v1/HTTP decibel endpoint, by contrast, wants only `dba`/`dbc` and discards the extremes — worth flagging in the spec if the richer payload is to survive.

**Spec-relevant constraints cheat-sheet:** one JSON object per POST; 1 MiB body cap; `float32`-safe finite values; `occupancy_count` whole 0…2³²−1; RFC3339 UTC; `venue_id`/`location` must not be sent (ignored if they are); unknown keys tolerated; 202 ≠ durable; auth failures indistinguishable (401); type mismatch (403); status result closed set.

---

Task complete — reporting success.