//! PMS5003 — Plantower laser particulate sensor over UART
//! (`sensor-pms5003` → `pm25`).
//!
//! Protocol per the Plantower PMS5003 datasheet, cross-checked against the
//! V5 variant's PMS5003 driver (same constants): frames are 32 bytes —
//! start pair `0x42 0x4D`, a 16-bit frame-length field counting
//! 2×13 + 2 (28), thirteen big-endian words, and a checksum that is the
//! 16-bit sum of the frame's first 30 bytes. The first six words carry
//! PM1.0 / PM2.5 / PM10.0 twice: CF=1 calibration (industrial equivalence)
//! and atmospheric calibration. The reading published here is the
//! **atmospheric** PM2.5 — the datasheet's ambient-air concentration, the
//! same word the V5 driver publishes, so both variants report one number
//! for one air mass. The count words ride along in [`PmsFrame`] for
//! diagnostics but have no ingest topic of their own.
//!
//! The driver runs the sensor in **passive mode**: init disarms the
//! power-up streaming loop (`0xE1` / data `0x0000`), and each `sample`
//! issues one single-read request (`0xE2`) and drains the UART until a
//! frame is complete. Request-per-sample means the byte stream is never
//! free-running — reassembly only ever sees what this task asked for,
//! and a silent sensor surfaces as a read timeout instead of an
//! indefinitely parked future.
//!
//! Frame reassembly is a byte-oriented hunt: any byte pair that is not
//! the start pair is skipped, so garbage on the line, command
//! acknowledgements, and corrupt frames all resync without human help.
//! A corrupt-but-aligned frame fails its checksum — reported as a fault
//! so supervision sees the sensor degrade, rather than silently skipped.
//!
//! Error codes (`SensorError.code`, this driver's causes):
//!
//! | code | fault | meaning |
//! |------|-------|---------|
//! | 1 | Bus | UART read failed (timeout or line error) |
//! | 2 | Bus | single-read request write failed |
//! | 3 | Reading | frame checksum mismatch |
//! | 4 | Reading | no valid frame within the read budget |
//! | sensor's own | Hardware | frame error-code byte nonzero (1 = fan
//!   abnormal, 2 = laser abnormal, …) — passed through |

use crate::events::{MeasurementPayload, Pm25Sample};
use crate::sensor::{SensorDriver, SensorError, SensorFault};
use crate::sensors::Wait;
use crate::sensors::uart::{UartError, UartPort};

// Error codes — see the module table.
const ERR_READ: u8 = 1;
const ERR_WRITE: u8 = 2;
const ERR_CHECKSUM: u8 = 3;
const ERR_NO_FRAME: u8 = 4;

/// Passive mode (`0xE1`, data `0x0000`): stop streaming, answer only
/// single-read requests. Checksum is the 16-bit sum of the first five
/// bytes.
const CMD_PASSIVE: [u8; 7] = [0x42, 0x4D, 0xE1, 0x00, 0x00, 0x01, 0x70];
/// Single read in passive mode (`0xE2`, data `0x0000`). The reply is one
/// standard data frame.
const CMD_READ: [u8; 7] = [0x42, 0x4D, 0xE2, 0x00, 0x00, 0x01, 0x71];

/// One command-execution window before treating the port as quiescent
/// (datasheet command table).
const MODE_WAIT_MS: u64 = 100;

/// Start-of-frame pair: the ASCII characters 'B', 'M'.
const START: [u8; 2] = [0x42, 0x4D];
/// Total frame length: 4 header bytes + 26 payload + 2 checksum.
const FRAME_LEN: usize = 32;
/// The frame-length field's only valid value for a data frame: 2×13 + 2.
const PAYLOAD_LEN: u16 = 28;

/// Bytes the port is drained into per read call.
const READ_BUF_LEN: usize = 64;
/// Read budget per sample, in bytes. A single frame is 32; the budget
/// absorbs stray ack frames and misaligned garbage while bounding a
/// pathological stream to a few reads instead of an infinite loop.
const READ_BUDGET_BYTES: usize = 512;

/// The checksum of a PMS5003 frame or command: the 16-bit sum of every
/// byte before the checksum field.
#[must_use]
fn checksum(bytes: &[u8]) -> u16 {
    bytes
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)))
}

/// Why an assembled frame was rejected. The parser resyncs either way;
/// the reason decides whether the driver treats it as a sensor fault
/// (checksum — the sensor's own frame lied) or as noise (length — the
/// stream was never a data frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// The frame-length field was not 28 — a command acknowledgement,
    /// a lost byte reinterpreted, or garbage that happened to align.
    Length(u16),
    /// The checksum did not cover the frame as received.
    Checksum,
}

/// One parsed frame, in the sensor's own units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmsFrame {
    pub pm1_0_cf1: u16,
    pub pm2_5_cf1: u16,
    pub pm10_cf1: u16,
    pub pm1_0_atm: u16,
    pub pm2_5_atm: u16,
    pub pm10_atm: u16,
    /// Counts per 0.1 L of air: 0.3, 0.5, 1.0, 2.5, 5.0, 10 µm bins.
    pub counts: [u16; 6],
    /// Firmware version reported by the sensor.
    pub version: u8,
    /// Sensor self-diagnosis: 0 = healthy, 1 = fan abnormal, 2 = laser
    /// abnormal, …
    pub error_code: u8,
}

/// What one byte contributed to reassembly: nothing yet, a complete
/// frame, or a rejected attempt (the parser has already reset and is
/// hunting the next start pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parsed {
    Frame(PmsFrame),
    Rejected(Reject),
}

/// The byte-oriented frame reassembler. Push bytes as the port delivers
/// them, in any chunking; it hunts the start pair, buffers a frame's
/// worth, validates length and checksum, and yields the frame.
#[derive(Debug)]
pub struct FrameParser {
    buffer: [u8; FRAME_LEN],
    filled: usize,
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameParser {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: [0; FRAME_LEN],
            filled: 0,
        }
    }

    /// Forget any partially assembled frame — after a line-level fault
    /// the buffered bytes are untrustworthy, so reassembly starts over.
    pub fn reset(&mut self) {
        self.filled = 0;
    }

    /// How many bytes of the current candidate frame are buffered.
    #[must_use]
    pub const fn filled(&self) -> usize {
        self.filled
    }

    /// Feed one byte. Returns [`Parsed::Frame`] once a checksum-valid
    /// data frame is complete, [`Parsed::Rejected`] when a candidate was
    /// discarded (parser already reset), or `None` while the frame is
    /// still assembling.
    pub fn push(&mut self, byte: u8) -> Option<Parsed> {
        match self.filled {
            // Hunting: only the start pair begins a candidate. A 0x42
            // that turns out to be data can itself be the start of the
            // next pair (0x42 0x42 0x4D), so it is retained.
            0 => {
                if byte == START[0] {
                    self.buffer[0] = byte;
                    self.filled = 1;
                }
                None
            }
            1 => {
                if byte == START[1] {
                    self.buffer[1] = byte;
                    self.filled = 2;
                } else if byte != START[0] {
                    self.filled = 0;
                }
                None
            }
            // The length field decides whether what follows can be a
            // data frame at all. Anything but 28 is discarded whole.
            2 | 3 => {
                self.buffer[self.filled] = byte;
                self.filled += 1;
                if self.filled == 4 {
                    let declared = u16::from_be_bytes([self.buffer[2], self.buffer[3]]);
                    if declared == PAYLOAD_LEN {
                        None
                    } else {
                        self.filled = 0;
                        Some(Parsed::Rejected(Reject::Length(declared)))
                    }
                } else {
                    None
                }
            }
            // Payload and checksum accumulate to a full frame, which is
            // then checksummed. One bad frame costs nothing but the
            // wait for the next start pair.
            _ => {
                self.buffer[self.filled] = byte;
                self.filled += 1;
                if self.filled == FRAME_LEN {
                    self.filled = 0;
                    let frame = &self.buffer;
                    let covered = &frame[..FRAME_LEN - 2];
                    let expected = u16::from_be_bytes([frame[30], frame[31]]);
                    if checksum(covered) != expected {
                        return Some(Parsed::Rejected(Reject::Checksum));
                    }
                    return Some(Parsed::Frame(Self::parse(frame)));
                }
                None
            }
        }
    }

    /// Words out of a checksum-validated frame. Private: only reached
    /// with `frame` a full, verified 32-byte buffer.
    fn parse(frame: &[u8; FRAME_LEN]) -> PmsFrame {
        let word = |at: usize| u16::from_be_bytes([frame[at], frame[at + 1]]);
        PmsFrame {
            pm1_0_cf1: word(4),
            pm2_5_cf1: word(6),
            pm10_cf1: word(8),
            pm1_0_atm: word(10),
            pm2_5_atm: word(12),
            pm10_atm: word(14),
            counts: [word(16), word(18), word(20), word(22), word(24), word(26)],
            version: frame[28],
            error_code: frame[29],
        }
    }
}

/// The PMS5003 driver: port handle + wait seam, one reassembler.
pub struct Pms5003<U, W> {
    port: U,
    waiter: W,
    parser: FrameParser,
    scratch: [u8; READ_BUF_LEN],
}

impl<U, W> Pms5003<U, W> {
    /// Build a driver over `port`, timing command execution with
    /// `waiter`.
    #[must_use]
    pub const fn new(port: U, waiter: W) -> Self {
        Self {
            port,
            waiter,
            parser: FrameParser::new(),
            scratch: [0; READ_BUF_LEN],
        }
    }

    fn bus_fault(error: UartError, code: u8) -> SensorError {
        let _ = error;
        SensorError::new(SensorFault::Bus, code)
    }
}

impl<U: UartPort, W: Wait> SensorDriver for Pms5003<U, W> {
    async fn init(&mut self) -> Result<(), SensorError> {
        // A warm reboot does not power-cycle the sensor: whatever mode a
        // previous run left it in, passive is re-armed every boot. The
        // streaming loop a stale active mode would keep running is the
        // failure this prevents — free-running frames interleaved with
        // the driver's own read requests.
        self.port
            .write(&CMD_PASSIVE)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_WRITE))?;
        self.waiter.wait(MODE_WAIT_MS).await;
        Ok(())
    }

    async fn calibrate(&mut self) -> Result<(), SensorError> {
        // Factory-calibrated laser counter; the framework's per-unit
        // hook is a no-op.
        Ok(())
    }

    async fn sample(&mut self) -> Result<MeasurementPayload, SensorError> {
        // Ask for one frame, then drain until reassembly yields it. The
        // budget bounds the drain: a sensor that answers but never with
        // a valid data frame faults instead of spinning.
        self.parser.reset();
        self.port
            .write(&CMD_READ)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_WRITE))?;

        let mut budget = READ_BUDGET_BYTES;
        while budget > 0 {
            let room = self.scratch.len().min(budget);
            let read = self.port.read(&mut self.scratch[..room]).await;
            let len = match read {
                Ok(len) if len > 0 => len,
                // Ok(0) is not a success shape on this seam (see
                // `UartPort::read`); treat it as the silence it is.
                Ok(_) => return Err(Self::bus_fault(UartError::Timeout, ERR_READ)),
                Err(error) => {
                    self.parser.reset();
                    return Err(Self::bus_fault(error, ERR_READ));
                }
            };
            budget -= len;

            for &byte in &self.scratch[..len] {
                match self.parser.push(byte) {
                    Some(Parsed::Frame(frame)) => return reading(frame),
                    Some(Parsed::Rejected(Reject::Checksum)) => {
                        return Err(SensorError::new(SensorFault::Reading, ERR_CHECKSUM));
                    }
                    // Acknowledgements and misaligned garbage resync in
                    // the parser; they are not sensor faults.
                    Some(Parsed::Rejected(Reject::Length(_))) => {}
                    None => {}
                }
            }
        }
        Err(SensorError::new(SensorFault::Reading, ERR_NO_FRAME))
    }
}

/// Map a validated frame onto the ingest vocabulary: the sensor's own
/// error bits are a hardware fault carrying the sensor's code; otherwise
/// the atmospheric PM2.5 concentration is the reading.
fn reading(frame: PmsFrame) -> Result<MeasurementPayload, SensorError> {
    if frame.error_code != 0 {
        return Err(SensorError::new(SensorFault::Hardware, frame.error_code));
    }
    Ok(MeasurementPayload::Pm25(Pm25Sample {
        ug_m3: f32::from(frame.pm2_5_atm),
    }))
}

#[cfg(test)]
mod tests {
    use heapless::Vec as HeapVec;

    use super::*;

    /// The datasheet-shaped fixture frames (checksums precomputed; see
    /// `tests/pms5003_frame_fixtures.rs` for the whole-flow versions).
    /// PM2.5 CF=1 = 100, atmospheric = 87 µg/m³.
    const FRAME_GOOD: [u8; 32] = [
        0x42, 0x4D, 0x00, 0x1C, 0x00, 0x2D, 0x00, 0x64, 0x00, 0xC8, 0x00, 0x23, 0x00,
        0x57, 0x00, 0xDC, 0x07, 0x53, 0x02, 0x8F, 0x01, 0x90, 0x00, 0x64, 0x00, 0x0A,
        0x00, 0x04, 0x1C, 0x00, 0x05, 0x64,
    ];

    fn feed(parser: &mut FrameParser, bytes: &[u8]) -> Option<Parsed> {
        let mut last = None;
        for &byte in bytes {
            last = parser.push(byte);
        }
        last
    }

    /// Concatenate byte chunks into one stream (the maximum any of these
    /// scripts needs is 42 bytes).
    fn stream(chunks: &[&[u8]]) -> HeapVec<u8, 64> {
        let mut script = HeapVec::new();
        for chunk in chunks {
            script
                .extend_from_slice(chunk)
                .expect("script capacity is a test size");
        }
        script
    }

    #[test]
    fn checksum_is_the_16bit_sum_of_the_covered_bytes() {
        assert_eq!(checksum(&[0x42, 0x4D, 0xE1, 0x00, 0x00]), 0x0170);
        assert_eq!(checksum(&[0xFF, 0xFF]), 0x01FE);
        // Wrapping is part of the contract: 30 × 255 sums to 7650, a
        // u16, exactly as the sensor computes it.
        assert_eq!(checksum(&[0xFF; 30]), 0x1DE2);
    }

    #[test]
    fn a_frame_arriving_byte_by_byte_parses_to_the_atmospheric_word() {
        let mut parser = FrameParser::new();
        assert_eq!(
            feed(&mut parser, &FRAME_GOOD),
            Some(Parsed::Frame(PmsFrame {
                pm1_0_cf1: 45,
                pm2_5_cf1: 100,
                pm10_cf1: 200,
                pm1_0_atm: 35,
                pm2_5_atm: 87,
                pm10_atm: 220,
                counts: [1875, 655, 400, 100, 10, 4],
                version: 0x1C,
                error_code: 0,
            }))
        );
        // A parsed frame leaves the parser ready for the next one.
        assert_eq!(parser.filled(), 0);
    }

    #[test]
    fn noise_before_the_start_pair_is_skipped() {
        let mut parser = FrameParser::new();
        let script = stream(&[&[0x00, 0xFF, 0x00, 0x00], &FRAME_GOOD]);
        assert!(matches!(feed(&mut parser, &script), Some(Parsed::Frame(_))));
    }

    #[test]
    fn a_false_start_hunts_on_without_losing_the_real_pair() {
        // 0x42 that is not followed by 0x4D is noise; a 0x42 followed by
        // another 0x42 keeps the second as the candidate start.
        let mut parser = FrameParser::new();
        let script = stream(&[&[0x42, 0x00, 0x42, 0x42, 0x4D], &FRAME_GOOD[2..]]);
        assert!(matches!(feed(&mut parser, &script), Some(Parsed::Frame(_))));
    }

    #[test]
    fn a_non_data_frame_is_rejected_by_its_length_field_and_resyncs() {
        let mut parser = FrameParser::new();
        let ack = [0x42, 0x4D, 0x00, 0x04, 0xE1, 0x00, 0x00, 0x01, 0x01, 0x75];
        let script = stream(&[&ack, &FRAME_GOOD]);
        assert_eq!(
            feed(&mut parser, &script),
            Some(Parsed::Frame(PmsFrame {
                pm1_0_cf1: 45,
                pm2_5_cf1: 100,
                pm10_cf1: 200,
                pm1_0_atm: 35,
                pm2_5_atm: 87,
                pm10_atm: 220,
                counts: [1875, 655, 400, 100, 10, 4],
                version: 0x1C,
                error_code: 0,
            }))
        );
    }

    #[test]
    fn a_corrupt_frame_is_rejected_on_its_checksum_and_resyncs() {
        let mut corrupt = FRAME_GOOD;
        corrupt[6] ^= 0x01; // one flipped bit in the PM2.5 CF=1 word
        let mut parser = FrameParser::new();
        let outcome = feed(&mut parser, &corrupt);
        assert_eq!(outcome, Some(Parsed::Rejected(Reject::Checksum)));
        assert_eq!(parser.filled(), 0);

        // The next frame parses cleanly — the reject consumed exactly
        // the bad candidate.
        assert!(matches!(
            feed(&mut parser, &FRAME_GOOD),
            Some(Parsed::Frame(_))
        ));
    }

    #[test]
    fn reset_discards_a_half_assembled_frame() {
        let mut parser = FrameParser::new();
        for &byte in &FRAME_GOOD[..10] {
            let _ = parser.push(byte);
        }
        assert_eq!(parser.filled(), 10);
        parser.reset();
        assert_eq!(parser.filled(), 0);
        // After the reset the same bytes parse as a fresh frame.
        assert!(matches!(
            feed(&mut parser, &FRAME_GOOD),
            Some(Parsed::Frame(_))
        ));
    }
}
