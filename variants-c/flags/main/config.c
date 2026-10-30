//! Runtime configuration loader — the implementation of config.h.
//!
//! Tier order (spec, tested on the host): built-in constants are written
//! first, a parseable default.toml overwrites them key by key, and the
//! override source overwrites the result. Every known key validates before
//! it lands; a value that fails validation (wrong type, out of range,
//! string too long) is skipped with a WARN_BAD_VALUE entry and the lower
//! tier's value stands. Keys the schema does not know are skipped with
//! WARN_UNKNOWN_KEY. The pool struct simply has no members for secrets, so
//! neither tier can smuggle one in and no dump can emit one.

#include "config.h"

#include <inttypes.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "toml.h"

/** Where the TOML tier's text is staged before parsing. tomlc99's
 * toml_parse takes a mutable char* and the device's embedded default.toml
 * lives in flash, so the text is copied here; files beyond the cap are
 * refused (ERR_TOML), never parsed partially. */
#define CLUBINS_CONFIG_TOML_SCRATCH 2048

static char toml_scratch[CLUBINS_CONFIG_TOML_SCRATCH];
static clubins_config_t config_pool;
static bool config_pool_loaded;

/* ------------------------------------------------------------------ */
/* Warn list                                                           */
/* ------------------------------------------------------------------ */

static void warn_push(clubins_config_warn_t *warn, const char *key, clubins_config_warn_kind_t kind)
{
    if (warn == NULL) {
        return;
    }
    if (warn->count >= CLUBINS_CONFIG_MAX_WARN) {
        warn->truncated = true;
        return;
    }
    clubins_config_warn_entry_t *entry = &warn->entries[warn->count++];
    snprintf(entry->key, sizeof(entry->key), "%s", key);
    entry->kind = kind;
}

static void warn_reset(clubins_config_warn_t *warn)
{
    if (warn != NULL) {
        memset(warn, 0, sizeof(*warn));
    }
}

/** Full dotted path of `key` under `section` ("sampling.report_interval_s";
 * "<root>" fields pass a section of NULL). */
static void dotted_path(char *out, size_t cap, const char *section, const char *key)
{
    if (section == NULL) {
        snprintf(out, cap, "%s", key);
    } else {
        snprintf(out, cap, "%s.%s", section, key);
    }
}

const char *clubins_config_warn_kind_name(clubins_config_warn_kind_t kind)
{
    switch (kind) {
    case CLUBINS_CONFIG_WARN_UNKNOWN_KEY:
        return "unknown-key";
    case CLUBINS_CONFIG_WARN_BAD_VALUE:
        return "bad-value";
    case CLUBINS_CONFIG_WARN_SOURCE_ERROR:
        return "source-error";
    default:
        return NULL;
    }
}

/* ------------------------------------------------------------------ */
/* Validation — one predicate per constrained field. Tolerance contract:  */
/* a failing predicate means the tier's value is skipped, never clamped.  */
/* ------------------------------------------------------------------ */

static bool valid_offline_buffer(int32_t v)
{
    return v >= 0 && v <= 4096;
}

static bool valid_decibel_window_ms(int32_t v)
{
    return v > 0 && v <= 60000;
}

static bool valid_report_interval_s(int32_t v)
{
    return v > 0 && v <= 86400;
}

/** 7-bit I2C addressable range. */
static bool valid_i2c_address(int32_t v)
{
    return v >= 0x08 && v <= 0x77;
}

/** ESP32-S3 GPIO number. */
static bool valid_gpio_pin(int32_t v)
{
    return v >= 0 && v <= 48;
}

static bool valid_mqtt_port(int32_t v)
{
    return v >= 1 && v <= 65535;
}

static bool valid_overload_threshold_db(double v)
{
    return v >= 0.0 && v <= 200.0;
}

static bool valid_calibration_offset_db(double v)
{
    return v >= -100.0 && v <= 100.0;
}

/** UUID-shaped identity: 1..36 chars of [0-9A-Za-z-] (the built-in
 * placeholder and real UUIDs both match; NVS provisioning writes the real
 * one). */
static bool valid_sensor_id(const char *s)
{
    size_t len = strlen(s);
    if (len == 0 || len >= CLUBINS_CONFIG_SENSOR_ID_LEN) {
        return false;
    }
    for (size_t i = 0; i < len; i++) {
        char c = s[i];
        bool ok =
            (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '-';
        if (!ok) {
            return false;
        }
    }
    return true;
}

/** Absolute http(s) URL: scheme:// then something host-shaped. Plain http
 * stays valid (host testing, local gateways); TLS enforcement is the
 * transport task's job. */
static bool valid_http_endpoint(const char *s)
{
    const char *rest = NULL;
    if (strncmp(s, "https://", 8) == 0) {
        rest = s + 8;
    } else if (strncmp(s, "http://", 7) == 0) {
        rest = s + 7;
    } else {
        return false;
    }
    return *rest != '\0';
}

static bool valid_mqtt_broker(const char *s)
{
    return s[0] != '\0' && strlen(s) < CLUBINS_CONFIG_BROKER_LEN;
}

/** Exact transport vocabulary (case-sensitive, like every other value). */
static bool parse_transport(const char *s, clubins_transport_active_t *out)
{
    if (strcmp(s, "http") == 0) {
        *out = CLUBINS_TRANSPORT_HTTP;
        return true;
    }
    if (strcmp(s, "mqtt") == 0) {
        *out = CLUBINS_TRANSPORT_MQTT;
        return true;
    }
    return false;
}

/* ------------------------------------------------------------------ */
/* TOML tier                                                           */
/* ------------------------------------------------------------------ */

/** Warn about every key of `tab` that is not in `known` (NUL-separated
 * list ends at NULL), under dotted prefix `section`. */
static void scan_unknown_keys(const toml_table_t *tab, const char *section,
                              const char *const *known, size_t nknown, clubins_config_warn_t *warn)
{
    for (int i = 0;; i++) {
        const char *key = toml_key_in(tab, i);
        if (key == NULL) {
            break;
        }
        bool recognized = false;
        for (size_t j = 0; j < nknown; j++) {
            if (strcmp(key, known[j]) == 0) {
                recognized = true;
                break;
            }
        }
        if (!recognized) {
            char path[64];
            dotted_path(path, sizeof(path), section, key);
            warn_push(warn, path, CLUBINS_CONFIG_WARN_UNKNOWN_KEY);
        }
    }
}

static void toml_apply_i32(const toml_table_t *tab, const char *section, const char *key,
                           int32_t *dst, bool (*valid)(int32_t), clubins_config_warn_t *warn)
{
    toml_datum_t datum = toml_int_in(tab, key);
    if (!datum.ok) {
        if (toml_key_exists(tab, key)) {
            char path[64];
            dotted_path(path, sizeof(path), section, key);
            warn_push(warn, path, CLUBINS_CONFIG_WARN_BAD_VALUE);
        }
        return;
    }
    if (datum.u.i < INT32_MIN || datum.u.i > INT32_MAX ||
        (valid != NULL && !valid((int32_t)datum.u.i))) {
        char path[64];
        dotted_path(path, sizeof(path), section, key);
        warn_push(warn, path, CLUBINS_CONFIG_WARN_BAD_VALUE);
        return;
    }
    *dst = (int32_t)datum.u.i;
}

static void toml_apply_f64(const toml_table_t *tab, const char *section, const char *key,
                           double *dst, bool (*valid)(double), clubins_config_warn_t *warn)
{
    toml_datum_t datum = toml_double_in(tab, key);
    if (!datum.ok) {
        if (toml_key_exists(tab, key)) {
            char path[64];
            dotted_path(path, sizeof(path), section, key);
            warn_push(warn, path, CLUBINS_CONFIG_WARN_BAD_VALUE);
        }
        return;
    }
    if (valid != NULL && !valid(datum.u.d)) {
        char path[64];
        dotted_path(path, sizeof(path), section, key);
        warn_push(warn, path, CLUBINS_CONFIG_WARN_BAD_VALUE);
        return;
    }
    *dst = datum.u.d;
}

/** Apply a string field; `cap` is the pool buffer size including NUL — a
 * file value that would not fit is rejected, never truncated. */
static void toml_apply_str(const toml_table_t *tab, const char *section, const char *key, char *dst,
                           size_t cap, bool (*valid)(const char *), clubins_config_warn_t *warn)
{
    toml_datum_t datum = toml_string_in(tab, key);
    if (!datum.ok) {
        if (toml_key_exists(tab, key)) {
            char path[64];
            dotted_path(path, sizeof(path), section, key);
            warn_push(warn, path, CLUBINS_CONFIG_WARN_BAD_VALUE);
        }
        return;
    }
    bool fits = strlen(datum.u.s) < cap;
    bool ok = fits && (valid == NULL || valid(datum.u.s));
    if (ok) {
        snprintf(dst, cap, "%s", datum.u.s);
    } else {
        char path[64];
        dotted_path(path, sizeof(path), section, key);
        warn_push(warn, path, CLUBINS_CONFIG_WARN_BAD_VALUE);
    }
    free(datum.u.s);
}

/** Apply [transport] active; value is the string vocabulary parsed into
 * the pool's enum. */
static void toml_apply_transport(const toml_table_t *tab, const char *section,
                                 clubins_config_t *cfg, clubins_config_warn_t *warn)
{
    toml_datum_t datum = toml_string_in(tab, "active");
    if (!datum.ok) {
        if (toml_key_exists(tab, "active")) {
            char path[64];
            dotted_path(path, sizeof(path), section, "active");
            warn_push(warn, path, CLUBINS_CONFIG_WARN_BAD_VALUE);
        }
        return;
    }
    clubins_transport_active_t parsed = CLUBINS_TRANSPORT_COUNT;
    bool ok = parse_transport(datum.u.s, &parsed);
    if (ok) {
        cfg->transport_active = parsed;
    } else {
        char path[64];
        dotted_path(path, sizeof(path), section, "active");
        warn_push(warn, path, CLUBINS_CONFIG_WARN_BAD_VALUE);
    }
    free(datum.u.s);
}

/** Parse the staged TOML in toml_scratch and apply the file tier. Sets
 * *parsed to false on a parse error (the caller keeps built-ins). */
static void apply_toml_table(clubins_config_t *cfg, clubins_config_warn_t *warn, bool *parsed)
{
    char errbuf[160];
    toml_table_t *root = toml_parse(toml_scratch, errbuf, sizeof(errbuf));
    if (root == NULL) {
        warn_push(warn, "<file>", CLUBINS_CONFIG_WARN_BAD_VALUE);
        *parsed = false;
        return;
    }
    *parsed = true;

    static const char *const root_keys[] = {"sensor", "sampling", "sensors", "transport",
                                            "decibel"};
    scan_unknown_keys(root, NULL, root_keys, 5, warn);

    const toml_table_t *sensor = toml_table_in(root, "sensor");
    if (sensor != NULL) {
        static const char *const keys[] = {"id"};
        scan_unknown_keys(sensor, "sensor", keys, 1, warn);
        toml_apply_str(sensor, "sensor", "id", cfg->sensor_id, sizeof(cfg->sensor_id),
                       valid_sensor_id, warn);
    }

    const toml_table_t *sampling = toml_table_in(root, "sampling");
    if (sampling != NULL) {
        static const char *const keys[] = {"decibel_window_ms", "report_interval_s",
                                           "offline_buffer"};
        scan_unknown_keys(sampling, "sampling", keys, 3, warn);
        toml_apply_i32(sampling, "sampling", "decibel_window_ms", &cfg->decibel_window_ms,
                       valid_decibel_window_ms, warn);
        toml_apply_i32(sampling, "sampling", "report_interval_s", &cfg->report_interval_s,
                       valid_report_interval_s, warn);
        toml_apply_i32(sampling, "sampling", "offline_buffer", &cfg->offline_buffer,
                       valid_offline_buffer, warn);
    }

    const toml_table_t *sensors = toml_table_in(root, "sensors");
    if (sensors != NULL) {
        static const char *const keys[] = {"bme280", "pir"};
        scan_unknown_keys(sensors, "sensors", keys, 2, warn);
        const toml_table_t *bme280 = toml_table_in(sensors, "bme280");
        if (bme280 != NULL) {
            static const char *const bme_keys[] = {"i2c_address"};
            scan_unknown_keys(bme280, "sensors.bme280", bme_keys, 1, warn);
            toml_apply_i32(bme280, "sensors.bme280", "i2c_address", &cfg->bme280_i2c_address,
                           valid_i2c_address, warn);
        }
        const toml_table_t *pir = toml_table_in(sensors, "pir");
        if (pir != NULL) {
            static const char *const pir_keys[] = {"gpio_pin"};
            scan_unknown_keys(pir, "sensors.pir", pir_keys, 1, warn);
            toml_apply_i32(pir, "sensors.pir", "gpio_pin", &cfg->pir_gpio_pin, valid_gpio_pin,
                           warn);
        }
    }

    const toml_table_t *transport = toml_table_in(root, "transport");
    if (transport != NULL) {
        static const char *const keys[] = {"active", "http_endpoint", "mqtt"};
        scan_unknown_keys(transport, "transport", keys, 3, warn);
        toml_apply_transport(transport, "transport", cfg, warn);
        toml_apply_str(transport, "transport", "http_endpoint", cfg->http_endpoint,
                       sizeof(cfg->http_endpoint), valid_http_endpoint, warn);
        const toml_table_t *mqtt = toml_table_in(transport, "mqtt");
        if (mqtt != NULL) {
            static const char *const mqtt_keys[] = {"broker", "port"};
            scan_unknown_keys(mqtt, "transport.mqtt", mqtt_keys, 2, warn);
            toml_apply_str(mqtt, "transport.mqtt", "broker", cfg->mqtt_broker,
                           sizeof(cfg->mqtt_broker), valid_mqtt_broker, warn);
            toml_apply_i32(mqtt, "transport.mqtt", "port", &cfg->mqtt_port, valid_mqtt_port, warn);
        }
    }

    const toml_table_t *decibel = toml_table_in(root, "decibel");
    if (decibel != NULL) {
        static const char *const keys[] = {"overload_threshold_db", "calibration_offset_db"};
        scan_unknown_keys(decibel, "decibel", keys, 2, warn);
        toml_apply_f64(decibel, "decibel", "overload_threshold_db", &cfg->overload_threshold_db,
                       valid_overload_threshold_db, warn);
        toml_apply_f64(decibel, "decibel", "calibration_offset_db", &cfg->calibration_offset_db,
                       valid_calibration_offset_db, warn);
    }

    toml_free(root);
}

/* ------------------------------------------------------------------ */
/* Override tier (the injectable source — NVS on device, tables on host) */
/* ------------------------------------------------------------------ */

static void ov_apply_i32(const clubins_config_overrides_t *ov, const char *key, int32_t *dst,
                         bool (*valid)(int32_t), clubins_config_warn_t *warn)
{
    if (ov->get_i32 == NULL) {
        return;
    }
    int32_t value = 0;
    int rc = ov->get_i32(ov->ctx, key, &value);
    if (rc == 0) {
        return;
    }
    if (rc < 0 || (valid != NULL && !valid(value))) {
        warn_push(warn, key,
                  rc < 0 ? CLUBINS_CONFIG_WARN_SOURCE_ERROR : CLUBINS_CONFIG_WARN_BAD_VALUE);
        return;
    }
    *dst = value;
}

static void ov_apply_f64(const clubins_config_overrides_t *ov, const char *key, double *dst,
                         bool (*valid)(double), clubins_config_warn_t *warn)
{
    if (ov->get_f64 == NULL) {
        return;
    }
    double value = 0.0;
    int rc = ov->get_f64(ov->ctx, key, &value);
    if (rc == 0) {
        return;
    }
    if (rc < 0 || (valid != NULL && !valid(value))) {
        warn_push(warn, key,
                  rc < 0 ? CLUBINS_CONFIG_WARN_SOURCE_ERROR : CLUBINS_CONFIG_WARN_BAD_VALUE);
        return;
    }
    *dst = value;
}

static void ov_apply_str(const clubins_config_overrides_t *ov, const char *key, char *dst,
                         size_t cap, bool (*valid)(const char *), clubins_config_warn_t *warn)
{
    if (ov->get_str == NULL) {
        return;
    }
    char scratch[CLUBINS_CONFIG_BROKER_LEN];
    int rc = ov->get_str(ov->ctx, key, scratch, sizeof(scratch));
    if (rc == 0) {
        return;
    }
    if (rc < 0) {
        warn_push(warn, key, CLUBINS_CONFIG_WARN_SOURCE_ERROR);
        return;
    }
    /* An override longer than the pool field cannot be applied truthfully
     * — reject rather than truncate. */
    if (strlen(scratch) >= cap || (valid != NULL && !valid(scratch))) {
        warn_push(warn, key, CLUBINS_CONFIG_WARN_BAD_VALUE);
        return;
    }
    snprintf(dst, cap, "%s", scratch);
}

static void ov_apply_transport(const clubins_config_overrides_t *ov, clubins_config_t *cfg,
                               clubins_config_warn_t *warn)
{
    if (ov->get_str == NULL) {
        return;
    }
    char scratch[16];
    int rc = ov->get_str(ov->ctx, "transport.active", scratch, sizeof(scratch));
    if (rc == 0) {
        return;
    }
    clubins_transport_active_t parsed = CLUBINS_TRANSPORT_COUNT;
    if (rc < 0 || !parse_transport(scratch, &parsed)) {
        warn_push(warn, "transport.active",
                  rc < 0 ? CLUBINS_CONFIG_WARN_SOURCE_ERROR : CLUBINS_CONFIG_WARN_BAD_VALUE);
        return;
    }
    cfg->transport_active = parsed;
}

static void apply_overrides(clubins_config_t *cfg, const clubins_config_overrides_t *ov,
                            clubins_config_warn_t *warn)
{
    ov_apply_str(ov, "sensor.id", cfg->sensor_id, sizeof(cfg->sensor_id), valid_sensor_id, warn);
    ov_apply_i32(ov, "sampling.decibel_window_ms", &cfg->decibel_window_ms, valid_decibel_window_ms,
                 warn);
    ov_apply_i32(ov, "sampling.report_interval_s", &cfg->report_interval_s, valid_report_interval_s,
                 warn);
    ov_apply_i32(ov, "sampling.offline_buffer", &cfg->offline_buffer, valid_offline_buffer, warn);
    ov_apply_i32(ov, "sensors.bme280.i2c_address", &cfg->bme280_i2c_address, valid_i2c_address,
                 warn);
    ov_apply_i32(ov, "sensors.pir.gpio_pin", &cfg->pir_gpio_pin, valid_gpio_pin, warn);
    ov_apply_transport(ov, cfg, warn);
    ov_apply_str(ov, "transport.http_endpoint", cfg->http_endpoint, sizeof(cfg->http_endpoint),
                 valid_http_endpoint, warn);
    ov_apply_str(ov, "transport.mqtt.broker", cfg->mqtt_broker, sizeof(cfg->mqtt_broker),
                 valid_mqtt_broker, warn);
    ov_apply_i32(ov, "transport.mqtt.port", &cfg->mqtt_port, valid_mqtt_port, warn);
    ov_apply_f64(ov, "decibel.overload_threshold_db", &cfg->overload_threshold_db,
                 valid_overload_threshold_db, warn);
    ov_apply_f64(ov, "decibel.calibration_offset_db", &cfg->calibration_offset_db,
                 valid_calibration_offset_db, warn);
}

/* ------------------------------------------------------------------ */
/* Public API                                                          */
/* ------------------------------------------------------------------ */

void clubins_config_builtins(clubins_config_t *out)
{
    memset(out, 0, sizeof(*out));
    snprintf(out->sensor_id, sizeof(out->sensor_id), "%s", "00000000-0000-0000-0000-000000000000");
    out->decibel_window_ms = 1000; /* Fast windows (125 ms) live inside this */
    out->report_interval_s = 60;   /* per-type reporting cadence */
    out->offline_buffer = 64;      /* measurements held while disconnected */
    out->bme280_i2c_address = 0x76;
    out->pir_gpio_pin = 4;
    out->transport_active = CLUBINS_TRANSPORT_HTTP;
    snprintf(out->http_endpoint, sizeof(out->http_endpoint), "%s",
             "https://ingest.clubins.de/v1/sensors");
    snprintf(out->mqtt_broker, sizeof(out->mqtt_broker), "%s", "mqtt.clubins.de");
    out->mqtt_port = 8883; /* TLS listener; plain 1883 only without mTLS */
    out->overload_threshold_db = 120.0;
    out->calibration_offset_db = 0.0; /* per-unit value from NVS at runtime */
}

clubins_config_status_t clubins_config_load(const char *toml_text,
                                            const clubins_config_overrides_t *overrides,
                                            clubins_config_warn_t *warn)
{
    warn_reset(warn);
    if (toml_text == NULL || toml_text[0] == '\0') {
        /* File tier absent (fresh flash, NULL text): built-ins + overrides
         * still yield sane defaults. */
        clubins_config_builtins(&config_pool);
        if (overrides != NULL) {
            apply_overrides(&config_pool, overrides, warn);
        }
        config_pool_loaded = true;
        return CLUBINS_CONFIG_OK;
    }

    size_t len = strlen(toml_text);
    if (len >= sizeof(toml_scratch)) {
        warn_push(warn, "<file>", CLUBINS_CONFIG_WARN_BAD_VALUE);
        clubins_config_builtins(&config_pool);
        if (overrides != NULL) {
            apply_overrides(&config_pool, overrides, warn);
        }
        config_pool_loaded = true;
        return CLUBINS_CONFIG_ERR_TOML;
    }
    memcpy(toml_scratch, toml_text, len + 1);

    clubins_config_builtins(&config_pool);
    bool parsed = false;
    apply_toml_table(&config_pool, warn, &parsed);
    if (overrides != NULL) {
        apply_overrides(&config_pool, overrides, warn);
    }
    config_pool_loaded = true;
    return parsed ? CLUBINS_CONFIG_OK : CLUBINS_CONFIG_ERR_TOML;
}

const clubins_config_t *clubins_config_pool(void)
{
    return config_pool_loaded ? &config_pool : NULL;
}

bool clubins_config_equal(const clubins_config_t *a, const clubins_config_t *b)
{
    return strcmp(a->sensor_id, b->sensor_id) == 0 &&
           a->decibel_window_ms == b->decibel_window_ms &&
           a->report_interval_s == b->report_interval_s && a->offline_buffer == b->offline_buffer &&
           a->bme280_i2c_address == b->bme280_i2c_address && a->pir_gpio_pin == b->pir_gpio_pin &&
           a->transport_active == b->transport_active &&
           strcmp(a->http_endpoint, b->http_endpoint) == 0 &&
           strcmp(a->mqtt_broker, b->mqtt_broker) == 0 && a->mqtt_port == b->mqtt_port &&
           a->overload_threshold_db == b->overload_threshold_db &&
           a->calibration_offset_db == b->calibration_offset_db;
}

/* ------------------------------------------------------------------ */
/* Dump — the inverse of the file tier (round-trip + secret exclusion)  */
/* ------------------------------------------------------------------ */

typedef struct {
    char *buf;
    size_t cap;
    size_t used;
    bool overflow;
} dump_t;

static void dump_puts(dump_t *d, const char *s)
{
    if (d->overflow) {
        return;
    }
    size_t len = strlen(s);
    if (len >= d->cap - d->used) {
        d->overflow = true;
        return;
    }
    memcpy(d->buf + d->used, s, len);
    d->used += len;
    d->buf[d->used] = '\0';
}

static void dump_printf(dump_t *d, const char *fmt, ...)
{
    if (d->overflow) {
        return;
    }
    va_list args;
    va_start(args, fmt);
    int written = vsnprintf(d->buf + d->used, d->cap - d->used, fmt, args);
    va_end(args);
    if (written < 0 || (size_t)written >= d->cap - d->used) {
        d->overflow = true;
        return;
    }
    d->used += (size_t)written;
}

/** A lossless double rendering that TOML still reads as a float: %.17g,
 * with a ".0" appended when g-format produced an integer shape. */
static void dump_double(dump_t *d, double v)
{
    char tmp[40];
    snprintf(tmp, sizeof(tmp), "%.17g", v);
    if (strpbrk(tmp, ".eEni") == NULL) {
        dump_printf(d, "%s.0", tmp);
    } else {
        dump_puts(d, tmp);
    }
}

/** TOML basic string with the two escapable bytes handled. */
static void dump_quoted(dump_t *d, const char *s)
{
    dump_puts(d, "\"");
    for (const char *p = s; *p != '\0'; p++) {
        if (*p == '"' || *p == '\\') {
            char esc[3] = {'\\', *p, '\0'};
            dump_puts(d, esc);
        } else {
            char one[2] = {*p, '\0'};
            dump_puts(d, one);
        }
    }
    dump_puts(d, "\"");
}

int clubins_config_dump(const clubins_config_t *cfg, char *out, size_t cap)
{
    if (out == NULL || cap == 0) {
        return -1;
    }
    dump_t d = {.buf = out, .cap = cap, .used = 0, .overflow = false};
    out[0] = '\0';

    dump_puts(&d, "[sensor]\n");
    dump_puts(&d, "id = ");
    dump_quoted(&d, cfg->sensor_id);
    dump_puts(&d, "\n\n[sampling]\n");
    dump_printf(&d, "decibel_window_ms = %" PRIi32 "\n", cfg->decibel_window_ms);
    dump_printf(&d, "report_interval_s = %" PRIi32 "\n", cfg->report_interval_s);
    dump_printf(&d, "offline_buffer = %" PRIi32 "\n", cfg->offline_buffer);

    dump_puts(&d, "\n[sensors.bme280]\n");
    dump_printf(&d, "i2c_address = 0x%02" PRIx32 "\n", (uint32_t)cfg->bme280_i2c_address);
    dump_puts(&d, "\n[sensors.pir]\n");
    dump_printf(&d, "gpio_pin = %" PRIi32 "\n", cfg->pir_gpio_pin);

    dump_puts(&d, "\n[transport]\nactive = ");
    dump_quoted(&d, cfg->transport_active == CLUBINS_TRANSPORT_MQTT ? "mqtt" : "http");
    dump_puts(&d, "\nhttp_endpoint = ");
    dump_quoted(&d, cfg->http_endpoint);
    dump_puts(&d, "\n\n[transport.mqtt]\nbroker = ");
    dump_quoted(&d, cfg->mqtt_broker);
    dump_printf(&d, "\nport = %" PRIi32 "\n", cfg->mqtt_port);

    dump_puts(&d, "\n[decibel]\n");
    dump_puts(&d, "overload_threshold_db = ");
    dump_double(&d, cfg->overload_threshold_db);
    dump_puts(&d, "\ncalibration_offset_db = ");
    dump_double(&d, cfg->calibration_offset_db);
    dump_puts(&d, "\n");

    return d.overflow ? -1 : (int)d.used;
}
