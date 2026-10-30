//! Transport layer — moves [`Measurement`]s to the server.
//!
//! Compile-time flags select which transports exist in the binary; runtime
//! config (`transport.active`) picks which one runs. This is the flag
//! framework's transport half: `transport-http` and `transport-mqtt` are
//! independent leaf features, and the schedule builder wires only what is
//! compiled.

mod http;
#[cfg(feature = "transport-mqtt")]
mod mqtt;

pub use http::HttpTransport;
#[cfg(feature = "transport-mqtt")]
pub use mqtt::{MqttTransport, topic_for};

use crate::measurement::Measurement;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The offline buffer is full — the measurement is dropped and counted.
    QueueFull,
    /// The measurement could not be encoded for this transport.
    Encode,
    /// The transport has no active selection from config.
    NoTransport,
}

/// The transport contract, called from the comms phase only.
pub trait Transport {
    /// Queue one measurement. Non-blocking; bounded by the offline buffer.
    fn publish(&mut self, measurement: &Measurement) -> Result<(), TransportError>;

    /// Service the transport (connect, retry, drain) — must fit its comms
    /// phase budget; long work is deferred to the next poll.
    fn poll(&mut self, now: u64);

    /// Connected state, surfaced by the ops-status heartbeat.
    fn is_connected(&self) -> bool;
}
