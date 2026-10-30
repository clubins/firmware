//! Boot-scoped identity and per-topic sequencing — the session lifecycle of
//! `api/mqtt/topics.md`, shared by both transports.
//!
//! On boot the sensor generates a new `boot_id` (16 random bytes from the
//! hardware RNG) and resets the `seq` counter of every topic to 0. A
//! `boot_id` that repeats across connects means the sensor only
//! reconnected; a new one means it rebooted. `boot_id` + `seq` is also the
//! server's deduplication key, and the server derives the row id it stores
//! from that pair: `boot_id[0..12] ‖ be32(seq)` with the version nibble set
//! to 8 and the variant bits to `10` (UUIDv8, RFC 9562).
//!
//! Both transports speak that language. The MQTT path puts the pair in the
//! wire `Header` and the server derives the id; the HTTP path has no header,
//! so the encoder sends the derived id itself as the `measurement_id` /
//! `status_id` body field. Either way a measurement carries the same id for
//! the same (boot, seq).

use heapless::String as HeapString;

/// The boot this session belongs to: 16 random bytes, generated once per
/// boot. The device personality draws them from the hardware RNG at boot;
/// the transport tasks hand the value to every message they emit.
///
/// Stability is the contract, not secrecy: every consumer in one boot sees
/// the same bytes, so the type is `Copy` and equality is bytewise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootId([u8; 16]);

impl BootId {
    /// Wrap 16 bytes already drawn from a random source. No zero check: an
    /// all-zero boot_id is statistically what the RNG never produces, and a
    /// device that passes one has a broken RNG worth seeing, not hiding.
    #[must_use]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Per-topic message counter: 0, 1, 2, …, reset to 0 by every new boot
/// (topics.md session lifecycle).
///
/// Each publishing task owns one sequencer for its topic — one sensor task
/// per measurement type, ops-status for the status topic — so the counters
/// never race and no locking is needed. A gap within one boot_id is how the
/// server notices lost messages; two publishers sharing a type would
/// manufacture exactly those gaps, which is why ownership is one task.
#[derive(Debug)]
pub struct Sequencer {
    next: u32,
}

impl Sequencer {
    #[must_use]
    pub const fn new() -> Self {
        Self { next: 0 }
    }

    /// The next sequence number for the owning topic, starting at 0.
    ///
    /// Wraps at `u32::MAX` rather than panicking: 4.3 billion messages on
    /// one topic in one boot is unreachable for a battery sensor, and a
    /// saturating counter would turn an impossible overflow into a repeated
    /// dedup key — the one thing the pair must never be.
    pub fn next_seq(&mut self) -> u32 {
        let seq = self.next;
        self.next = self.next.wrapping_add(1);
        seq
    }
}

impl Default for Sequencer {
    fn default() -> Self {
        Self::new()
    }
}

/// The row id derived from the header pair, per topics.md: the first 12
/// bytes of `boot_id`, then `seq` as 4 big-endian bytes, with the version
/// nibble set to 8 and the variant bits to `10`.
///
/// The server computes this from an MQTT v2 `Header`; the HTTP codec sends
/// the same value as `measurement_id` (measurements) or `status_id`
/// (status). Messages on different topics can collide into the same id —
/// topics.md notes they land in different tables, so this is by design.
#[must_use]
pub fn derive_message_id(boot_id: &BootId, seq: u32) -> HeapString<36> {
    let mut bytes = [0u8; 16];
    bytes[..12].copy_from_slice(&boot_id.as_bytes()[..12]);
    bytes[12..].copy_from_slice(&seq.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x80; // version 8
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant `10`

    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut id = HeapString::new();
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-').expect("36 chars fit the id buffer");
        }
        id.push(HEX[usize::from(byte >> 4)] as char)
            .expect("36 chars fit the id buffer");
        id.push(HEX[usize::from(byte & 0x0f)] as char)
            .expect("36 chars fit the id buffer");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequencer_starts_at_zero_and_counts_up() {
        let mut seq = Sequencer::new();
        assert_eq!(seq.next_seq(), 0);
        assert_eq!(seq.next_seq(), 1);
        assert_eq!(seq.next_seq(), 2);
    }

    #[test]
    fn sequencers_are_independent_per_topic() {
        let mut decibel = Sequencer::new();
        let mut pax = Sequencer::new();
        assert_eq!(decibel.next_seq(), 0);
        assert_eq!(pax.next_seq(), 0);
        assert_eq!(pax.next_seq(), 1);
        assert_eq!(decibel.next_seq(), 1);
    }

    #[test]
    fn boot_id_stays_stable_across_the_session() {
        // One boot_id is created per boot and handed out by copy: every
        // consumer in the session must see the same bytes.
        let boot = BootId::new([
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10,
        ]);
        let handed_to_transport = boot;
        let handed_to_status = boot;
        assert_eq!(handed_to_transport.as_bytes(), boot.as_bytes());
        assert_eq!(handed_to_status, boot);

        // Derived ids inherit the leading bytes: same boot, same prefix,
        // differing only in the seq tail.
        let first = derive_message_id(&boot, 0);
        let second = derive_message_id(&boot, 1);
        assert_eq!(&first.as_str()[..28], "01020304-0506-8708-890a-0b0c");
        assert_eq!(second.as_str(), "01020304-0506-8708-890a-0b0c00000001");

        // A different boot must not collide on the prefix.
        let mut other = *boot.as_bytes();
        other[0] ^= 0xff;
        let rebooted = derive_message_id(&BootId::new(other), 0);
        assert_ne!(&rebooted.as_str()[..8], &first.as_str()[..8]);
    }

    #[test]
    fn derived_ids_match_the_topics_md_vectors() {
        // boot_id[0..12] ‖ be32(seq), version nibble 8, variant bits 10 —
        // vectors recomputed independently (RFC 9562 UUIDv8 layout).
        let boot = BootId::new([
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10,
        ]);
        assert_eq!(
            derive_message_id(&boot, 0).as_str(),
            "01020304-0506-8708-890a-0b0c00000000"
        );
        assert_eq!(
            derive_message_id(&boot, 1).as_str(),
            "01020304-0506-8708-890a-0b0c00000001"
        );
        assert_eq!(
            derive_message_id(&boot, 0x0102_0304).as_str(),
            "01020304-0506-8708-890a-0b0c01020304"
        );
        // seq overwrites only the last 4 bytes; boot bytes 12..16 are unused.
        let nil = BootId::new([0u8; 16]);
        assert_eq!(
            derive_message_id(&nil, 0).as_str(),
            "00000000-0000-8000-8000-000000000000"
        );
        assert_eq!(
            derive_message_id(&nil, 1).as_str(),
            "00000000-0000-8000-8000-000000000001"
        );
    }

    #[test]
    fn derived_id_is_canonical_uuid_shape() {
        let id = derive_message_id(&BootId::new([0xab; 16]), 0xdead_beef);
        assert_eq!(id.len(), 36);
        for (index, ch) in id.chars().enumerate() {
            if matches!(index, 8 | 13 | 18 | 23) {
                assert_eq!(ch, '-', "dash at {index}");
            } else {
                assert!(ch.is_ascii_hexdigit(), "hex digit at {index}");
            }
        }
        // Version nibble 8 lives in the third group's first hex digit,
        // variant `10` in the fourth group's first hex digit (8 or 9, a or b).
        assert_eq!(&id.as_str()[14..15], "8");
        assert!(matches!(&id.as_str()[19..20], "8" | "9" | "a" | "b"));
    }
}
