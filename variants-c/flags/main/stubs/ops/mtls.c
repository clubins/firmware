//! Stub: mTLS client certificates from NVS (CN = sensor UUID).
//!
//! Compiled in by CONFIG_CLUBINS_CAP_TRANSPORT_MTLS (which requires the
//! MQTT transport); certificate CN is the sensor UUID, per
//! api/mqtt/topics.md.

#include "stubkit.h"

void clubins_stub_mtls_init(void)
{
    /* Real NVS cert loading + mbedtls wiring lands with the mTLS task. */
}

void clubins_stub_mtls_tick(void)
{
    /* Real identity refresh lands with the mTLS task. */
}
