//! The Sensirion wire protocol bits shared by the SHT4x and SCD4x drivers:
//! the per-word CRC-8 and the big-endian 16-bit word parse.
//!
//! CRC parameters per the Sensirion datasheets (SHT4x §4.9, SCD4x §3.11):
//! polynomial 0x31, initial value 0xFF, MSB first, no reflection, no final
//! XOR. Every 16-bit data word on the wire is followed by its CRC byte; a
//! word whose CRC does not match is a corrupted transfer, never a reading.

/// Sensirion CRC-8 over `data` (poly 0x31, init 0xFF, no reflect/xor).
#[must_use]
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0xFF;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x31
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A 16-bit big-endian word and its CRC byte, checked as one unit.
///
/// Returns `None` when the CRC does not match — the caller turns that into
/// a bus-integrity fault. The check runs before any conversion so a
/// corrupted word can never become a plausible reading.
#[must_use]
pub fn checked_word(data: &[u8], offset: usize) -> Option<u16> {
    let word = [*data.get(offset)?, *data.get(offset + 1)?];
    let crc = *data.get(offset + 2)?;
    if crc8(&word) == crc {
        Some(u16::from_be_bytes(word))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_matches_the_sensirion_worked_example() {
        // Sensirion's published example: payload 0xBEEF → CRC 0x92 (the
        // worked example the SHT4x/SCD4x datasheets and application notes
        // use to describe the algorithm).
        assert_eq!(crc8(&[0xBE, 0xEF]), 0x92);
    }

    #[test]
    fn crc8_pins_corner_inputs() {
        // Generated with an independent implementation of the same
        // parameters (poly 0x31, init 0xFF, MSB first) in the vector
        // generator; pins the bit loop against transcription slips.
        assert_eq!(crc8(&[0x00, 0x00]), 0x81);
        assert_eq!(crc8(&[0xFF, 0xFF]), 0xAC);
        assert_eq!(crc8(&[0xFD]), 0x62);
        assert_eq!(crc8(&[0x12, 0x34]), 0x37);
        assert_eq!(crc8(&[0x66, 0x66]), 0x93);
        assert_eq!(crc8(&[0x80, 0x00]), 0xA2);
    }

    #[test]
    fn checked_word_accepts_a_valid_word_and_rejects_a_corrupt_one() {
        // Word 0x0320 followed by its CRC 0x2A; flipping one data bit
        // breaks the check.
        let good = [0x03, 0x20, 0x2A];
        assert_eq!(checked_word(&good, 0), Some(800));

        let corrupt = [0x03, 0x21, 0x2A];
        assert_eq!(checked_word(&corrupt, 0), None);
        let bad_crc = [0x03, 0x20, 0x2B];
        assert_eq!(checked_word(&bad_crc, 0), None);
    }

    #[test]
    fn checked_word_never_panics_on_short_input() {
        // Truncated transfers come from the bus layer as short slices, not
        // promises; every offset of a short buffer is None, not a panic.
        for len in 0..3 {
            let short = [0x03, 0x20, 0x2A];
            assert_eq!(checked_word(&short[..len], 0), None);
            assert_eq!(checked_word(&short[..len], usize::MAX), None);
        }
    }
}
