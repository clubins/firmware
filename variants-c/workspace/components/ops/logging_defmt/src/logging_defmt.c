//! Structured logging capability (the `logging-defmt`
//! flag) — the C port of the logging-defmt crate's
//! scaffold. Will give the composed components one logging
//! facade (ESP-IDF log + defmt-style structured records for
//! host replay). The sink lands with the logging task.

#include "logging_defmt.h"

void clubins_logging_defmt_init(void)
{
    /* Lifecycle stub: the real bring-up lands with the logging-defmt port's
     * implementation tasks. */
}

void clubins_logging_defmt_tick(void)
{
    /* Tick stub: the real background work lands with the logging-defmt port's
     * implementation tasks. */
}
