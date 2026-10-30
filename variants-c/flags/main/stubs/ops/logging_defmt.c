//! Stub: defmt-style structured logging transport (UART/USB-JTAG).
//!
//! Compiled in by CONFIG_CLUBINS_CAP_LOGGING_DEFMT; esp_log stays the
//! baseline either way.

#include "stubkit.h"

void clubins_stub_logging_defmt_init(void)
{
    /* Real defmt-style sink bring-up lands with the logging task. */
}

void clubins_stub_logging_defmt_tick(void)
{
    /* Real log flushing lands with the logging task. */
}
