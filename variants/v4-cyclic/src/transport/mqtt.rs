//! MQTT v2 transport — the designed-but-unbuilt server bridge contract.
//!
//! Implemented to `api/mqtt/topics.md` + `api/mqtt/v2/*.proto`: one topic per
//! measurement type (`v2/sensors/<uuid>/data/<type>`, QoS 1), retained status,
//! `seq` restarting at 0 per topic per boot, 16-byte `boot_id` from the
//! hardware RNG per boot, `Online` retained status on every connect, `Offline`
//! on clean disconnect, LWT `Died`.
//!
//! Stub: the broker client, TLS/mTLS, and protobuf encoders land with the
//! transport tasks; the topic rule below is already contract-exact.

use super::{Transport, TransportError};
use crate::measurement::Measurement;
use crate::sensor::IngestType;

/// `v2/sensors/<uuid>/data/<type>` — one topic per measurement type
/// (`topics.md` §Topic structure). The UUID is the client cert CN under mTLS.
pub fn topic_for(sensor_uuid: &str, ingest_type: IngestType) -> heapless::String<128> {
    let mut topic = heapless::String::new();
    // Bounded writes: 36-char UUID + fixed segments + type fit 128 with room.
    let _ = topic.push_str("v2/sensors/");
    let _ = topic.push_str(sensor_uuid);
    let _ = topic.push_str("/data/");
    let _ = topic.push_str(ingest_type.segment());
    topic
}

pub struct MqttTransport;

impl MqttTransport {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for MqttTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for MqttTransport {
    fn publish(&mut self, _measurement: &Measurement) -> Result<(), TransportError> {
        // Stub: protobuf encode + QoS 1 publish land with the transport task.
        Err(TransportError::Encode)
    }

    fn poll(&mut self, _now: u64) {
        // Stub: broker connect, retained status, LWT setup land with the task.
    }

    fn is_connected(&self) -> bool {
        false // Stub: no broker client yet.
    }
}
