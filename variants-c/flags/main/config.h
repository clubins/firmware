//! Runtime configuration — spec §The config file, C port of V1's config.rs.
//!
//! One static pool, three precedence tiers, one rule (host-tested):
//!
//!     NVS override > committed config/default.toml > built-in constant
//!
//! The file tier is parsed by vendored tomlc99 into the pool; the NVS tier
//! reaches the loader through an injectable override-source interface, so
//! host tests exercise the full precedence ladder without NVS. Secrets are
//! structural non-members of the pool — API key, Wi-Fi credentials, and
//! client certificate/key are read from NVS by their consumers (transport
//! tasks), never carried here, so no serialization of this struct can leak
//! them (spec: "the committed file holds placeholders and never secrets").
//!
//! Unknown keys and values that fail validation are tolerated: the lower
//! tier's value stands and the issue lands in a warn list. A config edit
//! tunes runtime behavior; it can never enable a capability that was not
//! compiled in — that is the Kconfig half of the framework (deliberate).
//!
//! Not thread-safe by design: load once at boot, before the loop timer
//! starts; the pool is read-only afterwards.

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Wire-size caps for string pool fields (bytes, NUL included). */
#define CLUBINS_CONFIG_SENSOR_ID_LEN 37 /* 36-char UUID + NUL */
#define CLUBINS_CONFIG_ENDPOINT_LEN 96
#define CLUBINS_CONFIG_BROKER_LEN 128

/** Upper bound of warn-list entries a load will collect. Past it, entries
 * are dropped and `truncated` is set — the list is a diagnostic, not a
 * protocol. */
#define CLUBINS_CONFIG_MAX_WARN 16

/** [transport] active — which compiled-in transport publishes. */
typedef enum {
    CLUBINS_TRANSPORT_HTTP = 0,
    CLUBINS_TRANSPORT_MQTT = 1,
    CLUBINS_TRANSPORT_COUNT,
} clubins_transport_active_t;

/** The static configuration pool: every key of the committed
 * config/default.toml (spec example), typed. No secret fields — see the
 * file header. */
typedef struct {
    /* [sensor] */
    char sensor_id[CLUBINS_CONFIG_SENSOR_ID_LEN];
    /* [sampling] */
    int32_t decibel_window_ms;
    int32_t report_interval_s;
    int32_t offline_buffer;
    /* [sensors.bme280] / [sensors.pir] — per-device hardware identity */
    int32_t bme280_i2c_address;
    int32_t pir_gpio_pin;
    /* [transport] */
    clubins_transport_active_t transport_active;
    char http_endpoint[CLUBINS_CONFIG_ENDPOINT_LEN];
    /* [transport.mqtt] */
    char mqtt_broker[CLUBINS_CONFIG_BROKER_LEN];
    int32_t mqtt_port;
    /* [decibel] */
    double overload_threshold_db;
    double calibration_offset_db;
} clubins_config_t;

/** What one warn-list entry complains about. */
typedef enum {
    CLUBINS_CONFIG_WARN_UNKNOWN_KEY = 0, /** key is not in the schema */
    CLUBINS_CONFIG_WARN_BAD_VALUE,       /** known key, value rejected */
    CLUBINS_CONFIG_WARN_SOURCE_ERROR,    /** override source failed on key */
    CLUBINS_CONFIG_WARN_KIND_COUNT,
} clubins_config_warn_kind_t;

/** One tolerated issue: the dotted key it concerns (e.g.
 * "transport.mqtt.port"; "<file>" for file-level problems) and the kind. */
typedef struct {
    char key[64];
    clubins_config_warn_kind_t kind;
} clubins_config_warn_entry_t;

typedef struct {
    clubins_config_warn_entry_t entries[CLUBINS_CONFIG_MAX_WARN];
    size_t count;
    bool truncated; /** more issues than CLUBINS_CONFIG_MAX_WARN */
} clubins_config_warn_t;

/** Load outcome. ERR_TOML still leaves a usable pool (built-ins + any
 * overrides applied) — tolerance is the contract, the status is the log. */
typedef enum {
    CLUBINS_CONFIG_OK = 0,
    CLUBINS_CONFIG_ERR_TOML,    /* file present but unparseable/oversized */
    CLUBINS_CONFIG_ERR_ARGS,    /* NULL out pointer */
} clubins_config_status_t;

/** Injectable override source — the seam that keeps NVS out of the host
 * tests (the port of V1's trait ConfigSource). The device binds the NVS
 * adapter (nvs_overrides.h); host tests bind a plain table.
 *
 * Key space: dotted paths mirroring the file, e.g. "sensor.id",
 * "sampling.report_interval_s", "transport.mqtt.broker". Backends map
 * those onto their own key space (NVS keys cap at 15 chars).
 *
 * Every getter returns 1 = found (value written through the out arg),
 * 0 = key absent, <0 = source error. Strings are copied, always
 * NUL-terminated, never beyond `cap`. A NULL overrides pointer means "no
 * overrides tier"; NULL getters are treated as absent. */
typedef struct {
    void *ctx; /* implementation handle (NVS namespace, test table, ...) */
    int (*get_str)(void *ctx, const char *key, char *out, size_t cap);
    int (*get_i32)(void *ctx, const char *key, int32_t *out);
    int (*get_f64)(void *ctx, const char *key, double *out);
} clubins_config_overrides_t;

/** Tier 0 — the built-in constants (spec example values). */
void clubins_config_builtins(clubins_config_t *out);

/** Build the pool: built-ins, then the TOML text (NULL/empty = file tier
 * absent), then the override source. Full contents land in the static
 * pool (read it back with clubins_config_pool). `warn` may be NULL to
 * discard diagnostics. */
clubins_config_status_t clubins_config_load(const char *toml_text,
                                            const clubins_config_overrides_t *overrides,
                                            clubins_config_warn_t *warn);

/** The loaded pool, or NULL before the first load. Device code reads this
 * after clubins_config_load in boot; treat it as read-only. */
const clubins_config_t *clubins_config_pool(void);

/** Serialize the pool back to TOML — the inverse of the file tier and the
 * basis of the round-trip and secret-exclusion tests. Writes at most `cap`
 * bytes including the NUL; returns the byte count excluding the NUL, or
 * -1 when the buffer was too small (contents unspecified but
 * NUL-terminated). Emits no secret fields because the pool has none. */
int clubins_config_dump(const clubins_config_t *cfg, char *out, size_t cap);

/** Field-wise comparison (doubles compare exact — dump is lossless, so
 * round-trips compare equal bit-for-bit). */
bool clubins_config_equal(const clubins_config_t *a, const clubins_config_t *b);

/** Stable name of a warn kind ("unknown-key", "bad-value",
 * "source-error"); NULL past the vocabulary. */
const char *clubins_config_warn_kind_name(clubins_config_warn_kind_t kind);

#ifdef __cplusplus
}
#endif
