//! Wi-Fi station bring-up (the `net-wifi` capability) — the C
//! port of the net-wifi crate's scaffold: the esp_wifi +
//! esp_event bring-up that every transport and OTA assumes.
//! Connection state, the event handler, and the static IP
//! policy land with the network task; the transport/ops
//! components gate their work on the link coming up via this
//! component's init/tick pair until then.

#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Wired-up marker for the net task: true once a
 * station link has been brought up at least once. The
 * scaffold never sets it; transports will gate on it. */
bool clubins_net_wifi_link_up(void);
/** Bring the capability up once at boot (the composition calls this for
 * every composed hook, in registry order). */
void clubins_net_wifi_init(void);

/** Periodic hook: one slice of the capability's background work per
 * composition tick (the composition calls this for every composed hook,
 * after sampling the sensors). */
void clubins_net_wifi_tick(void);

#ifdef __cplusplus
}
#endif
