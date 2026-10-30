//! PIR occupancy front-end (the `sensor-pax` capability) — the C port of
//! the driver-pax crate.
//!
//! No PIR hardware is documented in the source repo, so the driver
//! reports PIR detections honestly: a motion-event count, not a
//! head-count metric. The GPIO interrupt front-end lands with the bus
//! task; `clubins_pir_record_motion` is the hook its interrupt handler
//! will call.

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "sensor.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Static metadata (the port of driver_pax::META). */
extern const clubins_sensor_meta_t clubins_pir_meta;

/** The config convention for the PIR pin (config/default.toml's
 * `[sensors.pir] gpio_pin` — no PIR hardware documented). The composition
 * uses it until the config pool lands. */
#define CLUBINS_PIR_DEFAULT_GPIO_PIN 4

/** Driver instance state (the port of the PirSensor struct). The motion
 * counter is 32-bit for interrupt-side headroom; the wire field folds
 * into the measurement model's uint8 window ceiling at sample time. */
typedef struct {
    uint8_t gpio_pin;
    uint32_t motions_since_sample;
    bool initialised;
} clubins_pir_driver_t;

/** Construct the driver on the given GPIO pin (the port of
 * PirSensor::new). */
void clubins_pir_driver_new(clubins_pir_driver_t *driver);

/** The configured pin (the port of PirSensor::gpio_pin). */
uint8_t clubins_pir_gpio_pin(const clubins_pir_driver_t *driver);

/** Interrupt-side hook: one PIR trigger. Saturating — an interrupt storm
 * must never wrap the count. */
void clubins_pir_record_motion(clubins_pir_driver_t *driver);

/** The sensor ops — pass to clubins_registry_push with the driver as ctx. */
extern const clubins_sensor_ops_t clubins_pir_sensor_ops;

#ifdef __cplusplus
}
#endif
