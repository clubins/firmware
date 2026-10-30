//! The composed app — the public surface of the composition root (the
//! platform-agnostic half; no IDF headers).
//!
//! clubins_app_compose() is the port of V2's registry_from_features: one
//! `#if CONFIG_CLUBINS_CAP_*` arm per capability leaf, each registering
//! its component's driver or hook. The composition reads the same flags
//! main/CMakeLists.txt used to build the requirement graph, so a flag
//! wired to a component that was not compiled in is a link error — never
//! a null callback at boot.

#pragma once

#include <stdbool.h>
#include <stddef.h>

#include "registry.h"

#ifdef __cplusplus
extern "C" {
#endif

/** Build the composed app from the build's capability selection
 * (Kconfig-driven on device, -D-driven on host). Idempotent: the
 * composition is built on the first call. */
void clubins_app_compose(void);

/** The composed sensor registry; NULL before clubins_app_compose(). */
clubins_registry_t *clubins_app_registry(void);

/** Composed sensor driver count (== registry length after compose). */
size_t clubins_app_composed_sensors(void);

/** Composed transport/ops hook count. */
size_t clubins_app_composed_hooks(void);

/** Measurements delivered to the (scaffold) sink so far. The real publish
 * path — encode + transport — lands with the transport tasks. */
uint32_t clubins_app_sampled_measurements(void);

/** Bring-up: init every composed sensor, then every composed hook. The
 * device boot path and the host tests drive the same calls. */
void clubins_app_init_all(void);

/** The periodic tick: sample every composed sensor, then tick every
 * composed hook. Called from the FreeRTOS timer (device) or the test
 * harness (host); safe to drive repeatedly. */
void clubins_app_tick(void);

#ifdef __cplusplus
}
#endif
