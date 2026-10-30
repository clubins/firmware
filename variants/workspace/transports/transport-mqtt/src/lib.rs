//! MQTT v2 protobuf transport (the `transport-mqtt` capability), per
//! `api/mqtt/topics.md` + `api/mqtt/v2/*.proto` in the source monorepo.
//!
//! The broker connection and the publish lifecycle (retained `Online`
//! status on connect, `Offline` on clean disconnect, LWT `Died`) land with
//! the MQTT transport task; this scaffold pins the topic rules and the
//! per-topic sequence numbers.

#![cfg_attr(not(test), no_std)]

use core::fmt::Write;
use heapless::String;

/// Topic buffer: `v2/sensors/<uuid>/data/<type>` fits comfortably.
pub const TOPIC_BUFFER: usize = 96;

/// `v2/sensors/<uuid>/data/<type>` — one topic per measurement type, QoS 1.
pub fn data_topic(sensor_uuid: &str, data_type: &str) -> String<TOPIC_BUFFER> {
    let mut topic = String::<TOPIC_BUFFER>::new();
    core::write!(topic, "v2/sensors/{sensor_uuid}/data/{data_type}")
        .expect("topic buffer");
    topic
}

/// `v2/sensors/<uuid>/status` — published retained on every connect.
pub fn status_topic(sensor_uuid: &str) -> String<TOPIC_BUFFER> {
    let mut topic = String::<TOPIC_BUFFER>::new();
    core::write!(topic, "v2/sensors/{sensor_uuid}/status").expect("topic buffer");
    topic
}

/// Per-topic sequence numbers: one counter per topic, starting at 0 per
/// boot (`topics.md` "seq"). A plain array indexed by an interned topic id;
/// with one type per topic the counter vector stays tiny.
#[derive(Debug)]
pub struct SeqCounter {
    next: [u32; 8],
}

impl SeqCounter {
    pub fn new() -> Self {
        Self { next: [0; 8] }
    }

    /// The sequence number to stamp next, and the counter bump.
    pub fn take(&mut self, topic_index: usize) -> u32 {
        let seq = self.next[topic_index];
        self.next[topic_index] = seq.wrapping_add(1);
        seq
    }
}

impl Default for SeqCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// mTLS client credentials (the `transport-mtls` capability): the CN is the
/// sensor UUID, the certificate and key live only in NVS.
#[cfg(feature = "mtls")]
pub mod mtls {
    use heapless::String;

    /// PEM sizes for an EC client cert + key pair.
    pub const CERT_BUFFER: usize = 2048;

    /// A client certificate/key pair with a redacting Debug.
    pub struct ClientCertificate {
        cert_pem: String<CERT_BUFFER>,
        key_pem: String<CERT_BUFFER>,
    }

    /// An error for a certificate that does not fit the fixed-size buffers.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CertificateTooLong;

    impl ClientCertificate {
        pub fn new(cert_pem: &str, key_pem: &str) -> Result<Self, CertificateTooLong> {
            Ok(Self {
                cert_pem: String::try_from(cert_pem).map_err(|_| CertificateTooLong)?,
                key_pem: String::try_from(key_pem).map_err(|_| CertificateTooLong)?,
            })
        }

        pub fn cert_pem(&self) -> &str {
            &self.cert_pem
        }

        /// For the TLS stack only; never for logging.
        pub fn key_pem(&self) -> &str {
            &self.key_pem
        }
    }

    impl core::fmt::Debug for ClientCertificate {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("ClientCertificate")
                .field("cert_pem", &"<redacted>")
                .field("key_pem", &"<redacted>")
                .finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "1b64b413-1c1e-4a5b-9d4d-9e9ecb050cee";

    #[test]
    fn data_topics_follow_the_topics_md_pattern() {
        for data_type in ["decibel", "pax", "weather", "status"] {
            let topic = data_topic(UUID, data_type);
            assert_eq!(
                topic.as_str(),
                format!("v2/sensors/{UUID}/data/{data_type}"),
                "topic for {data_type}"
            );
        }
    }

    #[test]
    fn status_topic_is_per_sensor_and_not_per_type() {
        assert_eq!(
            status_topic(UUID).as_str(),
            format!("v2/sensors/{UUID}/status")
        );
    }

    #[test]
    fn seq_counters_are_per_topic_and_start_at_zero() {
        let mut counters = SeqCounter::new();
        // Interleave two topics; each counts from 0 independently.
        assert_eq!(counters.take(0), 0);
        assert_eq!(counters.take(1), 0);
        assert_eq!(counters.take(0), 1);
        assert_eq!(counters.take(1), 1);
        assert_eq!(counters.take(0), 2);
        assert_eq!(counters.take(3), 0);
    }

    #[cfg(feature = "mtls")]
    #[test]
    fn client_certificate_debug_is_redacted() {
        let certificate = ClientCertificate::new(
            "-----BEGIN CERTIFICATE-----",
            "-----BEGIN PRIVATE KEY-----",
        )
        .unwrap();
        let rendered = alloc::format!("{certificate:?}");
        assert_eq!(rendered.matches("<redacted>").count(), 2);
        assert!(!rendered.contains("-----BEGIN"));
    }
}
