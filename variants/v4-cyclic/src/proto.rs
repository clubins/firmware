//! Protobuf v2 wire types — field-for-field with `api/mqtt/v2/*.proto`.
//!
//! Encoding lands with the transport tasks (conformance-tested against the
//! `.proto` files on the host); these types fix the shapes now so drivers and
//! the executive can already carry them.

/// `v2/header.proto` — attached to every published message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Milliseconds since the epoch; 0 while SNTP is unsynchronized (the
    /// contract distinguishes "no clock" from "wrong clock").
    pub time_ms: u64,
    /// 16 bytes from the hardware RNG, stable within a boot.
    pub boot_id: [u8; 16],
    /// Per-topic sequence, restarting at 0 each boot.
    pub seq: u32,
}

/// `v2/status.proto` `ConnectionState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Online,
    Offline,
    Died,
}

/// `v2/status.proto` — published retained on connect; the LWT publishes `Died`.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub firmware_version: heapless::String<32>,
    pub connection_state: ConnectionState,
}
