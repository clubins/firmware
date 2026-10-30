//! HTTP JSON ingest — the working server path.
//!
//! Posts one JSON object per measurement to `<endpoint>/v1/sensors/<type>`
//! with fields exactly as the Go ingest structs define them
//! (`service/service.go`, `decibel.go`, `pax.go`, `weather.go`, `status.go`).
//! Failures back off exponentially and drain a bounded offline buffer when
//! connectivity returns.
//!
//! Stub: Wi-Fi station bring-up (net-wifi), the TLS client, and the JSON
//! encoder (field-exact goldens) land with the transport tasks.

use super::{Transport, TransportError};
use crate::measurement::Measurement;

/// Runtime selection from `config/default.toml` (`[transport]`).
pub struct HttpTransport {
    /// Ingest base URL, e.g. `https://ingest.clubins.de/v1/sensors`.
    endpoint: heapless::String<128>,
}

impl HttpTransport {
    pub fn new(endpoint: &str) -> Self {
        let mut buf = heapless::String::new();
        // Bounded copy: an over-long endpoint is a config bug — truncate and
        // let the first request fail visibly rather than panic at boot.
        let _ = buf.push_str(endpoint);
        Self { endpoint: buf }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl Transport for HttpTransport {
    fn publish(&mut self, _measurement: &Measurement) -> Result<(), TransportError> {
        // Stub: JSON body encoder + request queue land with the transport
        // task. Nothing is dropped silently — the offline buffer owns it.
        Err(TransportError::Encode)
    }

    fn poll(&mut self, _now: u64) {
        // Stub: connect/backoff/drain state machine lands with the task.
    }

    fn is_connected(&self) -> bool {
        false // Stub: no Wi-Fi yet.
    }
}
