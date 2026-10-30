//! HTTP JSON codec implementation — a caller-buffer JSON writer and a
//! strict flat-object reader, both allocation-free and float-free.
//!
//! Encoding renders the model's fixed-point integers as canonical decimal
//! (scale 1 → exactly one fractional digit: 700 tenths-dB is "70.0");
//! decoding parses numbers back with pure integer arithmetic and rejects —
//! never silently approximates — anything the model cannot hold exactly,
//! mirroring the server's own refuse-instead-of-guess validation style.

#include "http_json.h"

#include <string.h>

/* ------------------------------------------------------------------ */
/* Encoding — an appending buffer that counts every byte it would have */
/* written, so truncation is detected once, at the end, with the true  */
/* required length reported.                                           */
/* ------------------------------------------------------------------ */

typedef struct {
    char *buf;
    size_t cap;
    size_t len;
} ob_t;

static void ob_put_ch(ob_t *ob, char c)
{
    if (ob->len < ob->cap) {
        ob->buf[ob->len] = c;
    }
    ob->len++;
}

static void ob_put_mem(ob_t *ob, const char *s, size_t n)
{
    for (size_t i = 0; i < n; i++) {
        ob_put_ch(ob, s[i]);
    }
}

/** pow10 up to 1e9 — far past the scales this codec uses. */
static const uint32_t s_pow10[10] = {1u,        10u,       100u,       1000u,     10000u,
                                     100000u,   1000000u,  10000000u,  100000000u, 1000000000u};

/** Render an unsigned integer in plain decimal. */
static void ob_put_uint(ob_t *ob, uint32_t value)
{
    char digits[10]; /* u32 max is 4294967295 — ten digits */
    size_t n = 0;
    do {
        digits[n++] = (char)('0' + (value % 10u));
        value /= 10u;
    } while (value != 0u);
    for (size_t i = n; i > 0; i--) {
        ob_put_ch(ob, digits[i - 1]);
    }
}

/** Render `value` measured in units of 10^-scale as canonical decimal:
 * always exactly `scale` fractional digits (654,s1 → "65.4"; -52,s1 →
 * "-5.2"). Scales above 9 are a programming error; the codec uses 0 and 1. */
static void ob_put_fixed(ob_t *ob, int32_t value, unsigned scale)
{
    uint32_t mag;
    if (value < 0) {
        ob_put_ch(ob, '-');
        mag = (uint32_t)(-(int64_t)value);
    } else {
        mag = (uint32_t)value;
    }
    const uint32_t div = s_pow10[scale];
    ob_put_uint(ob, mag / div);
    if (scale > 0u) {
        ob_put_ch(ob, '.');
        /* Zero-pad the fraction to exactly `scale` digits. */
        uint32_t frac = mag % div;
        char digits[9];
        for (unsigned i = scale; i > 0u; i--) {
            digits[i - 1u] = (char)('0' + (frac % 10u));
            frac /= 10u;
        }
        ob_put_mem(ob, digits, scale);
    }
}

/** Write a JSON string literal (with quotes), escaping what must be
 * escaped: quote, backslash, and control characters. Bytes ≥ 0x20 pass
 * through raw — valid UTF-8 stays valid JSON. */
static void ob_put_json_string(ob_t *ob, const char *s)
{
    ob_put_ch(ob, '"');
    for (const unsigned char *p = (const unsigned char *)s; *p != 0u; p++) {
        switch (*p) {
        case '"':
            ob_put_mem(ob, "\\\"", 2);
            break;
        case '\\':
            ob_put_mem(ob, "\\\\", 2);
            break;
        case '\b':
            ob_put_mem(ob, "\\b", 2);
            break;
        case '\f':
            ob_put_mem(ob, "\\f", 2);
            break;
        case '\n':
            ob_put_mem(ob, "\\n", 2);
            break;
        case '\r':
            ob_put_mem(ob, "\\r", 2);
            break;
        case '\t':
            ob_put_mem(ob, "\\t", 2);
            break;
        default:
            if (*p < 0x20u) {
                /* \u00XX for the remaining control characters. */
                static const char hex[] = "0123456789abcdef";
                char esc[6] = {'\\', 'u', '0', '0', hex[(*p >> 4) & 0xFu], hex[*p & 0xFu]};
                ob_put_mem(ob, esc, 6);
            } else {
                ob_put_ch(ob, (char)*p);
            }
            break;
        }
    }
    ob_put_ch(ob, '"');
}

/* Renders `"key":` (quoted key, no spaces — compact JSON). */
static void ob_put_key(ob_t *ob, const char *key)
{
    ob_put_json_string(ob, key);
    ob_put_ch(ob, ':');
}

static void ob_put_string_field(ob_t *ob, const char *key, const char *value)
{
    ob_put_key(ob, key);
    ob_put_json_string(ob, (value != NULL) ? value : "");
}

/** The per-type bodies, in the Go structs' declaration order — credentials
 * first (service.go embeds them), then the id, then the type's fields. */
static void encode_credentials(ob_t *ob, const clubins_http_ingest_request_t *req)
{
    ob_put_ch(ob, '{');
    ob_put_string_field(ob, "sensor_id", req->sensor_id);
    ob_put_ch(ob, ',');
    ob_put_string_field(ob, "apikey", req->apikey);
    ob_put_ch(ob, ',');
    ob_put_string_field(ob, "time", req->time);
}

static void encode_decibel(ob_t *ob, const clubins_http_ingest_request_t *req)
{
    encode_credentials(ob, req);
    if (req->measurement_id != NULL) {
        ob_put_ch(ob, ',');
        ob_put_string_field(ob, "measurement_id", req->measurement_id);
    }
    ob_put_ch(ob, ',');
    ob_put_key(ob, "dba");
    ob_put_fixed(ob, req->measurement.as.decibel.dba, 1u);
    ob_put_ch(ob, ',');
    ob_put_key(ob, "dbc");
    ob_put_fixed(ob, req->measurement.as.decibel.dbc, 1u);
    ob_put_ch(ob, '}');
}

static void encode_pax(ob_t *ob, const clubins_http_ingest_request_t *req)
{
    encode_credentials(ob, req);
    if (req->measurement_id != NULL) {
        ob_put_ch(ob, ',');
        ob_put_string_field(ob, "measurement_id", req->measurement_id);
    }
    ob_put_ch(ob, ',');
    ob_put_key(ob, "occupancy_count");
    ob_put_uint(ob, req->measurement.as.pax.occupancy_count);
    ob_put_ch(ob, '}');
}

static void encode_weather(ob_t *ob, const clubins_http_ingest_request_t *req)
{
    const clubins_weather_meas_t *w = &req->measurement.as.weather;
    encode_credentials(ob, req);
    if (req->measurement_id != NULL) {
        ob_put_ch(ob, ',');
        ob_put_string_field(ob, "measurement_id", req->measurement_id);
    }
    /* Go declaration order: temperature, humidity, precipitation,
     * rain_probability, wind_speed, pressure. The virtual-sensor fields in
     * the middle have no device model fields — a device body never
     * carries them (spec §Decisions; the server gap is tracked). */
    if (w->has_temperature) {
        ob_put_ch(ob, ',');
        ob_put_key(ob, "temperature");
        ob_put_fixed(ob, w->temperature_c, 1u);
    }
    if (w->has_humidity) {
        ob_put_ch(ob, ',');
        ob_put_key(ob, "humidity");
        ob_put_uint(ob, w->humidity_pct);
    }
    if (w->has_pressure) {
        ob_put_ch(ob, ',');
        ob_put_key(ob, "pressure");
        ob_put_uint(ob, w->pressure_hpa);
    }
    ob_put_ch(ob, '}');
}

static void encode_status(ob_t *ob, const clubins_http_ingest_request_t *req)
{
    const clubins_status_meas_t *s = &req->measurement.as.status;
    encode_credentials(ob, req);
    ob_put_ch(ob, ',');
    ob_put_string_field(ob, "status_id", (req->status_id != NULL) ? req->status_id : "");
    ob_put_ch(ob, ',');
    ob_put_string_field(ob, "subsystem", s->subsystem);
    ob_put_ch(ob, ',');
    ob_put_string_field(ob, "result", clubins_status_result_name(s->result));
    ob_put_ch(ob, ',');
    ob_put_string_field(ob, "message", s->message);
    ob_put_ch(ob, '}');
}

clubins_http_json_result_t
clubins_http_json_encode_request(const clubins_http_ingest_request_t *req, char *buf, size_t cap,
                                 size_t *out_len)
{
    if (out_len != NULL) {
        *out_len = 0;
    }
    if (req == NULL || out_len == NULL || (cap != 0u && buf == NULL)) {
        return CLUBINS_HTTP_JSON_ERR_BAD_ARGUMENT;
    }
    /* The server unmarshals credentials into strings: JSON null there is a
     * 400, so a NULL credential is not encodable rather than a null. */
    if (req->sensor_id == NULL || req->apikey == NULL || req->time == NULL) {
        return CLUBINS_HTTP_JSON_ERR_NOT_ENCODABLE;
    }
    /* co2, pm25, light have no HTTP ingest endpoint — MQTT topics only. */
    if ((size_t)req->measurement.type >= CLUBINS_MEAS_TYPE_COUNT ||
        req->measurement.type == CLUBINS_MEAS_CO2 ||
        req->measurement.type == CLUBINS_MEAS_PM25 ||
        req->measurement.type == CLUBINS_MEAS_LIGHT) {
        return CLUBINS_HTTP_JSON_ERR_NOT_ENCODABLE;
    }

    ob_t ob = {.buf = buf, .cap = cap, .len = 0};
    switch (req->measurement.type) {
    case CLUBINS_MEAS_DECIBEL:
        encode_decibel(&ob, req);
        break;
    case CLUBINS_MEAS_PAX:
        encode_pax(&ob, req);
        break;
    case CLUBINS_MEAS_WEATHER:
        encode_weather(&ob, req);
        break;
    case CLUBINS_MEAS_STATUS:
        encode_status(&ob, req);
        break;
    default:
        return CLUBINS_HTTP_JSON_ERR_NOT_ENCODABLE;
    }

    *out_len = ob.len;
    if (ob.len > cap) {
        return CLUBINS_HTTP_JSON_ERR_TRUNCATED;
    }
    if (ob.len < cap) {
        buf[ob.len] = '\0';
    }
    return CLUBINS_HTTP_JSON_OK;
}

/* ------------------------------------------------------------------ */
/* Decoding — a cursor over the input; strict where the server is      */
/* strict (JSON syntax, known-key types), skipping where it skips      */
/* (unknown keys).                                                     */
/* ------------------------------------------------------------------ */

typedef struct {
    const char *p;
    const char *end;
} cur_t;

static void skip_ws(cur_t *c)
{
    while (c->p < c->end && (*c->p == ' ' || *c->p == '\t' || *c->p == '\n' || *c->p == '\r')) {
        c->p++;
    }
}

static bool eat(cur_t *c, char expect)
{
    skip_ws(c);
    if (c->p < c->end && *c->p == expect) {
        c->p++;
        return true;
    }
    return false;
}

static bool peek(cur_t *c, char expect)
{
    skip_ws(c);
    return c->p < c->end && *c->p == expect;
}

static int hex_val(unsigned char ch)
{
    if (ch >= '0' && ch <= '9') {
        return ch - '0';
    }
    if (ch >= 'a' && ch <= 'f') {
        return ch - 'a' + 10;
    }
    if (ch >= 'A' && ch <= 'F') {
        return ch - 'A' + 10;
    }
    return -1;
}

/** Encode one code point as UTF-8 into out[4]; returns its byte count. */
static size_t utf8_encode(uint32_t cp, char *out)
{
    if (cp <= 0x7Fu) {
        out[0] = (char)cp;
        return 1;
    }
    if (cp <= 0x7FFu) {
        out[0] = (char)(0xC0u | (cp >> 6));
        out[1] = (char)(0x80u | (cp & 0x3Fu));
        return 2;
    }
    if (cp <= 0xFFFFu) {
        out[0] = (char)(0xE0u | (cp >> 12));
        out[1] = (char)(0x80u | ((cp >> 6) & 0x3Fu));
        out[2] = (char)(0x80u | (cp & 0x3Fu));
        return 3;
    }
    out[0] = (char)(0xF0u | (cp >> 18));
    out[1] = (char)(0x80u | ((cp >> 12) & 0x3Fu));
    out[2] = (char)(0x80u | ((cp >> 6) & 0x3Fu));
    out[3] = (char)(0x80u | (cp & 0x3Fu));
    return 4;
}

/** Parse a string literal into a bounded buffer (NUL-terminated). Always
 * consumes the literal, even when it does not fit. */
static clubins_http_json_result_t parse_string(cur_t *c, char *out, size_t cap)
{
    if (!eat(c, '"')) {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
    }
    size_t n = 0;
    while (c->p < c->end) {
        const unsigned char ch = (unsigned char)*c->p;
        c->p++;
        if (ch == '"') {
            if (n < cap) {
                out[n] = '\0';
            }
            return (n < cap) ? CLUBINS_HTTP_JSON_OK : CLUBINS_HTTP_JSON_ERR_FIELD_TOO_LONG;
        }
        if (ch == '\\') {
            if (c->p >= c->end) {
                return CLUBINS_HTTP_JSON_ERR_MALFORMED;
            }
            const char esc = *c->p++;
            char decoded[4];
            size_t width = 1;
            switch (esc) {
            case '"':
                decoded[0] = '"';
                break;
            case '\\':
                decoded[0] = '\\';
                break;
            case '/':
                decoded[0] = '/';
                break;
            case 'b':
                decoded[0] = '\b';
                break;
            case 'f':
                decoded[0] = '\f';
                break;
            case 'n':
                decoded[0] = '\n';
                break;
            case 'r':
                decoded[0] = '\r';
                break;
            case 't':
                decoded[0] = '\t';
                break;
            case 'u': {
                uint32_t cp = 0;
                for (int i = 0; i < 4; i++) {
                    if (c->p >= c->end) {
                        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
                    }
                    const int digit = hex_val((unsigned char)*c->p++);
                    if (digit < 0) {
                        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
                    }
                    cp = (cp << 4) | (uint32_t)digit;
                }
                if (cp >= 0xD800u && cp <= 0xDBFFu) {
                    /* High surrogate: a well-formed pair encodes one
                     * astral code point; a lone one is malformed. */
                    if (c->end - c->p < 6 || c->p[0] != '\\' || c->p[1] != 'u') {
                        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
                    }
                    uint32_t low = 0;
                    for (int i = 0; i < 4; i++) {
                        const int digit = hex_val((unsigned char)c->p[2 + i]);
                        if (digit < 0) {
                            return CLUBINS_HTTP_JSON_ERR_MALFORMED;
                        }
                        low = (low << 4) | (uint32_t)digit;
                    }
                    c->p += 6;
                    if (low < 0xDC00u || low > 0xDFFFu) {
                        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
                    }
                    cp = 0x10000u + ((cp - 0xD800u) << 10) + (low - 0xDC00u);
                } else if (cp >= 0xDC00u && cp <= 0xDFFFu) {
                    return CLUBINS_HTTP_JSON_ERR_MALFORMED; /* lone low surrogate */
                }
                width = utf8_encode(cp, decoded);
                break;
            }
            default:
                return CLUBINS_HTTP_JSON_ERR_MALFORMED;
            }
            for (size_t i = 0; i < width; i++) {
                if (n < cap) {
                    out[n] = decoded[i];
                }
                n++;
            }
            continue;
        }
        if (ch < 0x20u) {
            /* The server's decoder rejects raw control characters. */
            return CLUBINS_HTTP_JSON_ERR_MALFORMED;
        }
        if (n < cap) {
            out[n] = (char)ch;
        }
        n++;
    }
    return CLUBINS_HTTP_JSON_ERR_MALFORMED; /* ran off the end: no closing quote */
}

/** Consume a string literal without copying it. */
static clubins_http_json_result_t skip_string(cur_t *c)
{
    if (!eat(c, '"')) {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
    }
    while (c->p < c->end) {
        const char ch = *c->p++;
        if (ch == '"') {
            return CLUBINS_HTTP_JSON_OK;
        }
        if (ch == '\\') {
            if (c->p >= c->end) {
                return CLUBINS_HTTP_JSON_ERR_MALFORMED;
            }
            c->p++;
        }
    }
    return CLUBINS_HTTP_JSON_ERR_MALFORMED;
}

/** Parse a JSON number in fixed-point: `scale` fractional digits are
 * meaningful, further digits must be zeros (else the value is not exactly
 * representable), exponent notation is refused outright. Pure integer
 * arithmetic — no float is ever constructed. */
static clubins_http_json_result_t parse_fixed_number(cur_t *c, unsigned scale, bool allow_negative,
                                                     int32_t *out)
{
    skip_ws(c);
    if (c->p < c->end &&
        (*c->p == '"' || *c->p == '{' || *c->p == '[' || *c->p == 't' || *c->p == 'f' ||
         *c->p == 'n')) {
        /* A value of another JSON type where the contract wants a number —
         * the server's own decode failure (encoding/json type error). */
        return CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH;
    }
    bool negative = false;
    if (c->p < c->end && *c->p == '-') {
        negative = true;
        c->p++;
        if (!allow_negative) {
            return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE;
        }
    }
    if (c->p >= c->end || *c->p < '0' || *c->p > '9') {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
    }
    /* JSON forbids leading zeros ("0" and "10", not "010"). */
    if (*c->p == '0' && (c->p + 1 < c->end) && c->p[1] >= '0' && c->p[1] <= '9') {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
    }
    int64_t mag = 0;
    while (c->p < c->end && *c->p >= '0' && *c->p <= '9') {
        if (mag > (INT64_MAX - 9) / 10) {
            return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE;
        }
        mag = mag * 10 + (*c->p - '0');
        c->p++;
    }
    if (eat(c, '.')) {
        if (c->p >= c->end || *c->p < '0' || *c->p > '9') {
            return CLUBINS_HTTP_JSON_ERR_MALFORMED;
        }
        unsigned taken = 0;
        while (c->p < c->end && *c->p >= '0' && *c->p <= '9') {
            const int digit = *c->p - '0';
            c->p++;
            if (taken < scale) {
                mag = mag * 10 + digit;
                taken++;
            } else if (digit != 0) {
                return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE; /* beyond the scale */
            }
        }
        mag *= (int64_t)s_pow10[scale - taken]; /* pad to the full scale */
    }
    if (c->p < c->end && (*c->p == 'e' || *c->p == 'E')) {
        return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE; /* exponent notation */
    }
    if (negative) {
        mag = -mag;
    }
    if (mag > INT32_MAX || mag < INT32_MIN) {
        return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE;
    }
    *out = (int32_t)mag;
    return CLUBINS_HTTP_JSON_OK;
}

/** Validate a number-shaped token and discard it (unknown fields). */
static clubins_http_json_result_t skip_number(cur_t *c)
{
    skip_ws(c);
    const char *start = c->p;
    while (c->p < c->end &&
           ((*c->p >= '0' && *c->p <= '9') || *c->p == '-' || *c->p == '+' || *c->p == '.' ||
            *c->p == 'e' || *c->p == 'E')) {
        c->p++;
    }
    bool saw_digit = false;
    for (const char *p = start; p < c->p; p++) {
        if (*p >= '0' && *p <= '9') {
            saw_digit = true;
            break;
        }
    }
    return (saw_digit && c->p > start) ? CLUBINS_HTTP_JSON_OK : CLUBINS_HTTP_JSON_ERR_MALFORMED;
}

static clubins_http_json_result_t skip_literal(cur_t *c, const char *literal)
{
    const size_t n = strlen(literal);
    if ((size_t)(c->end - c->p) >= n && memcmp(c->p, literal, n) == 0) {
        c->p += n;
        return CLUBINS_HTTP_JSON_OK;
    }
    return CLUBINS_HTTP_JSON_ERR_MALFORMED;
}

/** Skip any JSON value (objects and arrays recursively, bounded). */
static clubins_http_json_result_t skip_value(cur_t *c, unsigned depth)
{
    if (depth > 16u) {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED; /* pathological nesting */
    }
    skip_ws(c);
    if (c->p >= c->end) {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
    }
    switch (*c->p) {
    case '"':
        return skip_string(c);
    case 't':
        return skip_literal(c, "true");
    case 'f':
        return skip_literal(c, "false");
    case 'n':
        return skip_literal(c, "null");
    case '[': {
        c->p++;
        if (peek(c, ']')) {
            c->p++;
            return CLUBINS_HTTP_JSON_OK;
        }
        for (;;) {
            const clubins_http_json_result_t element = skip_value(c, depth + 1u);
            if (element != CLUBINS_HTTP_JSON_OK) {
                return element;
            }
            if (eat(c, ',')) {
                continue;
            }
            if (eat(c, ']')) {
                return CLUBINS_HTTP_JSON_OK;
            }
            return CLUBINS_HTTP_JSON_ERR_MALFORMED;
        }
    }
    case '{': {
        c->p++;
        if (peek(c, '}')) {
            c->p++;
            return CLUBINS_HTTP_JSON_OK;
        }
        for (;;) {
            skip_ws(c);
            const clubins_http_json_result_t key = skip_string(c);
            if (key != CLUBINS_HTTP_JSON_OK) {
                return key;
            }
            if (!eat(c, ':')) {
                return CLUBINS_HTTP_JSON_ERR_MALFORMED;
            }
            const clubins_http_json_result_t value = skip_value(c, depth + 1u);
            if (value != CLUBINS_HTTP_JSON_OK) {
                return value;
            }
            if (eat(c, ',')) {
                continue;
            }
            if (eat(c, '}')) {
                return CLUBINS_HTTP_JSON_OK;
            }
            return CLUBINS_HTTP_JSON_ERR_MALFORMED;
        }
    }
    default:
        return skip_number(c);
    }
}

/** Bounded copies for the view's string fields. */
static clubins_http_json_result_t parse_string_field(cur_t *c, char *out, size_t cap)
{
    skip_ws(c);
    if (c->p >= c->end || *c->p != '"') {
        return CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH;
    }
    return parse_string(c, out, cap);
}

static clubins_http_json_result_t parse_u16_field(cur_t *c, uint16_t *out)
{
    int32_t value = 0;
    clubins_http_json_result_t r = parse_fixed_number(c, 0u, false, &value);
    if (r != CLUBINS_HTTP_JSON_OK) {
        return r;
    }
    if (value < 0 || value > 65535) {
        return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE;
    }
    *out = (uint16_t)value;
    return CLUBINS_HTTP_JSON_OK;
}

static clubins_http_json_result_t parse_u8_field(cur_t *c, uint8_t *out)
{
    int32_t value = 0;
    clubins_http_json_result_t r = parse_fixed_number(c, 0u, false, &value);
    if (r != CLUBINS_HTTP_JSON_OK) {
        return r;
    }
    if (value < 0 || value > 255) {
        return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE;
    }
    *out = (uint8_t)value;
    return CLUBINS_HTTP_JSON_OK;
}

/** dba/dbc render at scale 1 (tenths of a dB) and decode back at the same
 * scale — the encoder/decoder pair must agree on the unit. */
static clubins_http_json_result_t parse_u16_tenths_field(cur_t *c, uint16_t *out)
{
    int32_t value = 0;
    clubins_http_json_result_t r = parse_fixed_number(c, 1u, false, &value);
    if (r != CLUBINS_HTTP_JSON_OK) {
        return r;
    }
    if (value < 0 || value > 65535) {
        return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE;
    }
    *out = (uint16_t)value;
    return CLUBINS_HTTP_JSON_OK;
}

static clubins_http_json_result_t parse_i16_tenths_field(cur_t *c, int16_t *out)
{
    int32_t value = 0;
    clubins_http_json_result_t r = parse_fixed_number(c, 1u, true, &value);
    if (r != CLUBINS_HTTP_JSON_OK) {
        return r;
    }
    if (value < -32768 || value > 32767) {
        return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE;
    }
    *out = (int16_t)value;
    return CLUBINS_HTTP_JSON_OK;
}

/** Validate a known-to-the-server, unknown-to-the-model numeric field
 * (precipitation, rain_probability, wind_speed): the server requires a
 * number, so a wrong type is still a mismatch — but the value is discarded. */
static clubins_http_json_result_t skip_server_number_field(cur_t *c)
{
    skip_ws(c);
    if (c->p >= c->end) {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
    }
    if (*c->p == '"') {
        return CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH;
    }
    if (*c->p == '{' || *c->p == '[') {
        return CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH;
    }
    if (*c->p == 't' || *c->p == 'f' || *c->p == 'n') {
        return CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH;
    }
    return skip_number(c);
}

/** The result vocabulary is a closed set (status.go): anything else is a
 * value the model cannot hold. */
static clubins_http_json_result_t parse_result_field(cur_t *c, clubins_status_result_t *out)
{
    char name[16];
    clubins_http_json_result_t r = parse_string_field(c, name, sizeof(name));
    if (r != CLUBINS_HTTP_JSON_OK) {
        return r;
    }
    if (!clubins_status_result_parse(name, out)) {
        return CLUBINS_HTTP_JSON_ERR_VALUE_UNREPRESENTABLE;
    }
    return CLUBINS_HTTP_JSON_OK;
}

/** Key classification for the request decoder. */
typedef enum {
    KEY_SENSOR_ID,
    KEY_APIKEY,
    KEY_TIME,
    KEY_MEASUREMENT_ID,
    KEY_STATUS_ID,
    KEY_SUBSYSTEM,
    KEY_MESSAGE,
    KEY_DBA,
    KEY_DBC,
    KEY_OCCUPANCY_COUNT,
    KEY_TEMPERATURE,
    KEY_HUMIDITY,
    KEY_PRESSURE,
    KEY_PRECIPITATION,
    KEY_RAIN_PROBABILITY,
    KEY_WIND_SPEED,
    KEY_RESULT,
    KEY_UNKNOWN,
} req_key_t;

static req_key_t classify_key(const char *key)
{
    static const struct {
        const char *name;
        req_key_t id;
    } table[] = {
        {"sensor_id", KEY_SENSOR_ID},         {"apikey", KEY_APIKEY},
        {"time", KEY_TIME},                   {"measurement_id", KEY_MEASUREMENT_ID},
        {"status_id", KEY_STATUS_ID},         {"subsystem", KEY_SUBSYSTEM},
        {"message", KEY_MESSAGE},             {"dba", KEY_DBA},
        {"dbc", KEY_DBC},                     {"occupancy_count", KEY_OCCUPANCY_COUNT},
        {"temperature", KEY_TEMPERATURE},     {"humidity", KEY_HUMIDITY},
        {"pressure", KEY_PRESSURE},           {"precipitation", KEY_PRECIPITATION},
        {"rain_probability", KEY_RAIN_PROBABILITY},
        {"wind_speed", KEY_WIND_SPEED},       {"result", KEY_RESULT},
    };
    for (size_t i = 0; i < sizeof(table) / sizeof(table[0]); i++) {
        if (strcmp(key, table[i].name) == 0) {
            return table[i].id;
        }
    }
    return KEY_UNKNOWN;
}

/** Which ingest type a key belongs to; for KEY_UNKNOWN: the current type. */
static bool key_belongs_to_type(req_key_t key, clubins_meas_type_t type)
{
    switch (key) {
    case KEY_DBA:
    case KEY_DBC:
        return type == CLUBINS_MEAS_DECIBEL;
    case KEY_OCCUPANCY_COUNT:
        return type == CLUBINS_MEAS_PAX;
    case KEY_TEMPERATURE:
    case KEY_HUMIDITY:
    case KEY_PRESSURE:
    case KEY_PRECIPITATION:
    case KEY_RAIN_PROBABILITY:
    case KEY_WIND_SPEED:
        return type == CLUBINS_MEAS_WEATHER;
    case KEY_STATUS_ID:
    case KEY_SUBSYSTEM:
    case KEY_RESULT:
    case KEY_MESSAGE:
        return type == CLUBINS_MEAS_STATUS;
    default:
        return true; /* credentials and unknown keys are type-agnostic */
    }
}

static clubins_meas_type_t key_type(req_key_t key)
{
    switch (key) {
    case KEY_DBA:
    case KEY_DBC:
        return CLUBINS_MEAS_DECIBEL;
    case KEY_OCCUPANCY_COUNT:
        return CLUBINS_MEAS_PAX;
    case KEY_TEMPERATURE:
    case KEY_HUMIDITY:
    case KEY_PRESSURE:
    case KEY_PRECIPITATION:
    case KEY_RAIN_PROBABILITY:
    case KEY_WIND_SPEED:
        return CLUBINS_MEAS_WEATHER;
    case KEY_STATUS_ID:
    case KEY_SUBSYSTEM:
    case KEY_RESULT:
    case KEY_MESSAGE:
        return CLUBINS_MEAS_STATUS;
    default:
        return CLUBINS_MEAS_STATUS; /* unreachable for classified keys */
    }
}

clubins_http_json_result_t
clubins_http_json_decode_request(const char *json, size_t len,
                                 clubins_http_ingest_request_view_t *out)
{
    if (out == NULL) {
        return CLUBINS_HTTP_JSON_ERR_BAD_ARGUMENT;
    }
    memset(out, 0, sizeof(*out));
    if (json == NULL) {
        return CLUBINS_HTTP_JSON_ERR_BAD_ARGUMENT;
    }
    cur_t cur = {.p = json, .end = json + len};
    if (!eat(&cur, '{')) {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
    }

    clubins_meas_type_t seen_type = (clubins_meas_type_t)CLUBINS_MEAS_TYPE_COUNT;
    clubins_http_json_result_t r = CLUBINS_HTTP_JSON_OK;

    if (peek(&cur, '}')) {
        cur.p++;
    } else {
        for (;;) {
            char key[24];
            r = parse_string(&cur, key, sizeof(key));
            const bool key_long = (r == CLUBINS_HTTP_JSON_ERR_FIELD_TOO_LONG);
            if (r != CLUBINS_HTTP_JSON_OK && !key_long) {
                return r;
            }
            if (!eat(&cur, ':')) {
                return CLUBINS_HTTP_JSON_ERR_MALFORMED;
            }
            const req_key_t id = key_long ? KEY_UNKNOWN : classify_key(key);

            /* One POST, one measurement type: a second family's key is a
             * mismatch, exactly as one endpoint's struct would never carry
             * another's fields. */
            if (seen_type == (clubins_meas_type_t)CLUBINS_MEAS_TYPE_COUNT &&
                (id == KEY_DBA || id == KEY_DBC || id == KEY_OCCUPANCY_COUNT ||
                 id == KEY_TEMPERATURE || id == KEY_HUMIDITY || id == KEY_PRESSURE ||
                 id == KEY_STATUS_ID || id == KEY_SUBSYSTEM || id == KEY_RESULT ||
                 id == KEY_MESSAGE)) {
                seen_type = key_type(id);
            } else if (!key_belongs_to_type(id, seen_type)) {
                return CLUBINS_HTTP_JSON_ERR_TYPE_MISMATCH;
            }

            clubins_measurement_t *m = &out->measurement;
            switch (id) {
            case KEY_SENSOR_ID:
                r = parse_string_field(&cur, out->sensor_id, sizeof(out->sensor_id));
                break;
            case KEY_APIKEY:
                r = parse_string_field(&cur, out->apikey, sizeof(out->apikey));
                break;
            case KEY_TIME:
                r = parse_string_field(&cur, out->time, sizeof(out->time));
                break;
            case KEY_MEASUREMENT_ID:
                r = parse_string_field(&cur, out->measurement_id, sizeof(out->measurement_id));
                out->has_measurement_id = (r == CLUBINS_HTTP_JSON_OK);
                break;
            case KEY_STATUS_ID:
                r = parse_string_field(&cur, out->status_id, sizeof(out->status_id));
                out->has_status_id = (r == CLUBINS_HTTP_JSON_OK);
                break;
            case KEY_SUBSYSTEM:
                r = parse_string_field(&cur, out->subsystem, sizeof(out->subsystem));
                break;
            case KEY_MESSAGE:
                r = parse_string_field(&cur, out->message, sizeof(out->message));
                break;
            case KEY_DBA:
                r = parse_u16_tenths_field(&cur, &m->as.decibel.dba);
                break;
            case KEY_DBC:
                r = parse_u16_tenths_field(&cur, &m->as.decibel.dbc);
                break;
            case KEY_OCCUPANCY_COUNT:
                r = parse_u8_field(&cur, &m->as.pax.occupancy_count);
                break;
            case KEY_TEMPERATURE:
                r = parse_i16_tenths_field(&cur, &m->as.weather.temperature_c);
                m->as.weather.has_temperature = (r == CLUBINS_HTTP_JSON_OK);
                break;
            case KEY_HUMIDITY:
                r = parse_u8_field(&cur, &m->as.weather.humidity_pct);
                m->as.weather.has_humidity = (r == CLUBINS_HTTP_JSON_OK);
                break;
            case KEY_PRESSURE:
                r = parse_u16_field(&cur, &m->as.weather.pressure_hpa);
                m->as.weather.has_pressure = (r == CLUBINS_HTTP_JSON_OK);
                break;
            case KEY_PRECIPITATION:
            case KEY_RAIN_PROBABILITY:
            case KEY_WIND_SPEED:
                r = skip_server_number_field(&cur);
                break;
            case KEY_RESULT:
                r = parse_result_field(&cur, &m->as.status.result);
                break;
            default:
                r = skip_value(&cur, 0u);
                break;
            }
            if (r != CLUBINS_HTTP_JSON_OK) {
                return r;
            }

            if (eat(&cur, ',')) {
                continue;
            }
            if (eat(&cur, '}')) {
                break;
            }
            return CLUBINS_HTTP_JSON_ERR_MALFORMED;
        }
    }

    skip_ws(&cur);
    if (cur.p != cur.end) {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED; /* trailing garbage */
    }
    /* Fix the type for empty-object or credentials-only bodies: no
     * type-specific key means the type stays unset. */
    if (seen_type != (clubins_meas_type_t)CLUBINS_MEAS_TYPE_COUNT) {
        out->measurement.type = seen_type;
        if (seen_type == CLUBINS_MEAS_STATUS) {
            /* Mirror the collected strings into the measurement so the
             * decoded status is complete; an empty view string — absent or
             * explicit "" — is the same wire fact, so NULL it. */
            out->measurement.as.status.subsystem =
                (out->subsystem[0] != '\0') ? out->subsystem : NULL;
            out->measurement.as.status.message = (out->message[0] != '\0') ? out->message : NULL;
        }
    }
    return CLUBINS_HTTP_JSON_OK;
}

clubins_http_json_result_t
clubins_http_json_decode_response(const char *json, size_t len, clubins_http_response_t *out)
{
    if (out == NULL) {
        return CLUBINS_HTTP_JSON_ERR_BAD_ARGUMENT;
    }
    memset(out, 0, sizeof(*out));
    if (json == NULL) {
        return CLUBINS_HTTP_JSON_ERR_BAD_ARGUMENT;
    }
    cur_t cur = {.p = json, .end = json + len};
    if (!eat(&cur, '{')) {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
    }
    out->kind = CLUBINS_HTTP_RESPONSE_UNKNOWN;

    if (peek(&cur, '}')) {
        cur.p++;
    } else {
        for (;;) {
            char key[8];
            clubins_http_json_result_t r = parse_string(&cur, key, sizeof(key));
            if (r == CLUBINS_HTTP_JSON_ERR_FIELD_TOO_LONG) {
                r = CLUBINS_HTTP_JSON_OK; /* an overlong key is simply unknown */
            }
            if (r != CLUBINS_HTTP_JSON_OK) {
                return r;
            }
            if (!eat(&cur, ':')) {
                return CLUBINS_HTTP_JSON_ERR_MALFORMED;
            }
            if (strcmp(key, "status") == 0) {
                out->kind = CLUBINS_HTTP_RESPONSE_STATUS;
                r = parse_string_field(&cur, out->text, sizeof(out->text));
            } else if (strcmp(key, "error") == 0) {
                out->kind = CLUBINS_HTTP_RESPONSE_ERROR;
                r = parse_string_field(&cur, out->text, sizeof(out->text));
            } else {
                r = skip_value(&cur, 0u);
            }
            if (r != CLUBINS_HTTP_JSON_OK) {
                return r;
            }
            if (eat(&cur, ',')) {
                continue;
            }
            if (eat(&cur, '}')) {
                break;
            }
            return CLUBINS_HTTP_JSON_ERR_MALFORMED;
        }
    }

    skip_ws(&cur); /* the server's json.Encoder appends a newline */
    if (cur.p != cur.end) {
        return CLUBINS_HTTP_JSON_ERR_MALFORMED;
    }
    return CLUBINS_HTTP_JSON_OK;
}
