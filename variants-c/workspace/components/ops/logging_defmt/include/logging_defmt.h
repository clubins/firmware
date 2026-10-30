//! Structured logging capability (the `logging-defmt`
//! flag) — the C port of the logging-defmt crate's
//! scaffold. Will give the composed components one logging
//! facade (ESP-IDF log + defmt-style structured records for
//! host replay). The sink lands with the logging task.

#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/** The sink lands with the logging task: the scaffold
 * exposes only the composition lifecycle. */
/** Bring the capability up once at boot (the composition calls this for
 * every composed hook, in registry order). */
void clubins_logging_defmt_init(void);

/** Periodic hook: one slice of the capability's background work per
 * composition tick (the composition calls this for every composed hook,
 * after sampling the sensors). */
void clubins_logging_defmt_tick(void);

#ifdef __cplusplus
}
#endif
