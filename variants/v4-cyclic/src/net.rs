//! Network plumbing — Wi-Fi station and the wall clock.
//!
//! Both transports depend on `net-wifi`; the clock contract applies to both:
//! SNTP after network join, timestamps 0 while unsynchronized so the server
//! can distinguish "no clock" from "wrong clock".
//!
//! Stub: the esp-rs Wi-Fi stack (controller, STA join, DHCP) and SNTP client
//! land with the net task.

/// Credentials live only in NVS — never in committed config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiCredentials {
    pub ssid: heapless::String<32>,
    pub passphrase: heapless::String<64>,
}

/// Station state machine seam — the comms phase polls this through the
/// transport's `is_connected`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StationState {
    #[default]
    Down,
    Joining,
    Up,
    /// Join failed; backoff before retry.
    Failed,
}

/// Wall-clock contract per both ingest contracts: `now_ms` is 0 until SNTP
/// syncs, and callers must treat 0 as "no clock", never as 1970.
#[derive(Clone, Copy, Debug, Default)]
pub struct WallClock {
    /// SNTP-synced epoch milliseconds; None until first sync.
    epoch_ms: Option<u64>,
}

impl WallClock {
    pub const fn new() -> Self {
        Self { epoch_ms: None }
    }

    pub const fn is_synced(&self) -> bool {
        self.epoch_ms.is_some()
    }

    /// Epoch ms, or 0 while unsynchronized — the contract's "no clock" value.
    pub const fn now_ms(&self) -> u64 {
        match self.epoch_ms {
            Some(ms) => ms,
            None => 0,
        }
    }

    /// SNTP sync callback seam.
    pub fn apply_sntp(&mut self, epoch_ms: u64) {
        self.epoch_ms = Some(epoch_ms);
    }
}
