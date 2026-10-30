//! HTTP JSON ingest stub — gated by the `transport-http` flag.
//!
//! Contract (server-infra/ingest/service/*.go in the source monorepo):
//! HTTPS POST of one JSON object per measurement to
//! `<endpoint>/v1/sensors/<type>`; common fields `sensor_id`, `apikey`,
//! `time`, then per-type fields (`measurement_id` + `dba`/`dbc` for
//! decibel, `occupancy_count` for pax, weather fields). Field-exact wire
//! encoding, offline buffer drain, and exponential backoff arrive with the
//! transport PR; the stub runs the publish accounting.

use crate::boundary::{Measurement, http_route};

#[derive(Debug)]
pub struct HttpTransportStub {
    /// Measurements handed to the encoder since boot.
    pub published: u32,
    /// Measurements parked for the offline drain since boot.
    pub buffered: u32,
}

impl HttpTransportStub {
    pub const fn new() -> Self {
        HttpTransportStub {
            published: 0,
            buffered: 0,
        }
    }

    /// Accounts one measurement against its ingest route. Returns the
    /// running published count.
    pub fn publish(&mut self, measurement: &Measurement) -> u32 {
        // Route contract: /v1/sensors/<type>; encoding + POST arrive with
        // the transport PR.
        let _route = http_route(measurement.ingest_type());
        self.published += 1;
        self.published
    }

    /// Offline path (no connectivity): parked for the bounded drain on
    /// reconnect.
    pub fn buffer_offline(&mut self) -> u32 {
        self.buffered += 1;
        self.buffered
    }
}

impl Default for HttpTransportStub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_counts_and_routes_per_contract() {
        let mut t = HttpTransportStub::new();
        let m = Measurement::Pax { occupancy_count: 3 };
        assert_eq!(t.publish(&m), 1);
        assert_eq!(t.published, 1);
        assert_eq!(http_route(m.ingest_type()).as_str(), "/v1/sensors/pax");
    }

    #[test]
    fn offline_measurements_buffer() {
        let mut t = HttpTransportStub::new();
        t.buffer_offline();
        assert_eq!(t.buffered, 1);
        assert_eq!(t.published, 0);
    }
}
