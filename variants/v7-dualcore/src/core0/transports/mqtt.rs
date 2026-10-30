//! MQTT v2 transport stub — gated by the `transport-mqtt` flag.
//!
//! Contract (api/mqtt/topics.md + api/mqtt/v2/*.proto in the source
//! monorepo): per-type data topics `v2/sensors/<uuid>/data/<type>` at
//! QoS 1; a retained Status on every connect; `boot_id` + per-topic `seq`
//! dedup. The protobuf encoder and the client lifecycle (connect →
//! retained status, disconnect → Offline, LWT → Died) arrive with the
//! transport PR; the stub runs the publish accounting over the contract's
//! topic mapping.

use crate::boundary::{Measurement, PLACEHOLDER_UUID, mqtt_topic};

#[derive(Debug)]
pub struct MqttTransportStub {
    /// Data messages published since boot.
    pub published: u32,
}

impl MqttTransportStub {
    pub const fn new() -> Self {
        MqttTransportStub { published: 0 }
    }

    /// Accounts one measurement against its per-type topic. Returns the
    /// running published count.
    pub fn publish(&mut self, measurement: &Measurement) -> u32 {
        // Topic contract: v2/sensors/<uuid>/data/<type> at QoS 1; the real
        // client replaces the placeholder UUID from NVS (mTLS CN) later.
        let _topic = mqtt_topic(PLACEHOLDER_UUID, measurement.ingest_type());
        self.published += 1;
        self.published
    }
}

impl Default for MqttTransportStub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_maps_measurements_to_per_type_topics() {
        let mut t = MqttTransportStub::new();
        assert_eq!(t.publish(&Measurement::Co2 { ppm: 812 }), 1);
        assert_eq!(
            mqtt_topic(PLACEHOLDER_UUID, "co2").as_str(),
            "v2/sensors/00000000-0000-0000-0000-000000000000/data/co2"
        );
    }
}
