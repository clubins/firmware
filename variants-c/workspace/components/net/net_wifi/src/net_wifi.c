//! Wi-Fi station bring-up (the `net-wifi` capability) — the C
//! port of the net-wifi crate's scaffold: the esp_wifi +
//! esp_event bring-up that every transport and OTA assumes.
//! Connection state, the event handler, and the static IP
//! policy land with the network task; the transport/ops
//! components gate their work on the link coming up via this
//! component's init/tick pair until then.

#include "net_wifi.h"

void clubins_net_wifi_init(void)
{
    /* Lifecycle stub: the real bring-up lands with the net-wifi port's
     * implementation tasks. */
}

void clubins_net_wifi_tick(void)
{
    /* Tick stub: the real background work lands with the net-wifi port's
     * implementation tasks. */
}

bool clubins_net_wifi_link_up(void)
{
    /* The scaffold brings no link up; the network task wires
     * the real event handler. */
    return false;
}
