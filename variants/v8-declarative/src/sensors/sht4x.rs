//! SHT4x source driver — gated by the `sensor-sht4x` flag (pulls
//! `driver-i2c`).
//!
//! Driver contract (spec §Sensor Expansion): I²C; temperature, humidity →
//! `weather` measurements (`Measurement::Environmental`). Pressure is not
//! a property of this part.
//!
//! The flow follows the SHT4x datasheet: unlike the BME280's register
//! file, the SHT4x is command-driven — the host issues a command byte,
//! waits out the conversion, then clocks out a fixed frame. Every 16-bit
//! word in a frame is followed by a Sensirion CRC-8 (poly 0x31, init
//! 0xFF; the datasheet's worked example maps `0xBEEF` to `0x92`), and a
//! frame with a bad checksum is rejected rather than converted.
//!
//! Beat shape: each engine beat performs one leg of the flow — bring-up
//! beats issue the soft reset and then the serial-number probe (a
//! CRC-checked frame; the next beat arrives long after the 1 ms reset
//! time), a Ready beat issues the high-precision measurement command
//! (`0xFD`, ≤ 8.3 ms conversion), and the following beat collects and
//! converts the 6-byte frame. No spinning, no timers: the pipeline's
//! cadence paces the datasheet's waits.
//!
//! Wire values are the datasheet's conversion formulas evaluated in f32
//! (`T = −45 + 175·S_T/65535` °C, `RH = −6 + 125·S_RH/65535` %). No
//! clamping: the formulas ARE the contract, and the model's finite check
//! cannot fire on bounded integer inputs.

use crate::bus::{BusError, I2c};
use crate::measurement::Measurement;
use crate::sensors::{BeatDriver, SensorFault};

impl BeatDriver for Sht4x {
    fn beat<B: I2c>(&mut self, bus: &mut B) -> Result<Option<Measurement>, SensorFault> {
        // The specific error is the driver's own diagnostic (its tests pin
        // it); the engine counts faults, it does not classify them.
        self.measure(bus).map_err(|_| SensorFault)
    }
}

/// I²C address (all SHT4x variants; no address strap on this part).
pub const I2C_ADDRESS: u8 = 0x44;

// -- Commands (SHT4x datasheet) -------------------------------------------

/// High-precision measurement, longest conversion (~8.3 ms).
const CMD_MEASURE_HIGH: u8 = 0xFD;
/// Read the electronic identifier (serial number).
const CMD_SERIAL: u8 = 0x89;
/// Soft reset (restores the state of §3 defaults).
const CMD_SOFT_RESET: u8 = 0x94;

/// Every frame is two 16-bit words, each followed by its CRC-8.
const FRAME_LEN: usize = 6;

/// Sensirion CRC-8: polynomial 0x31 (x⁸ + x⁵ + x⁴ + 1), initial value
/// 0xFF, no reflection, no final xor. Bit-serial on purpose — a 256-entry
/// table costs flash the budget spends elsewhere.
fn crc8(data: &[u8]) -> u8 {
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

/// Raw ticks → °C. The datasheet's conversion is
/// `T = −45 + 175 · S_T / 65535`.
fn celsius(raw: u16) -> f32 {
    -45.0 + 175.0 * f32::from(raw) / 65_535.0
}

/// Raw ticks → %RH. The datasheet's conversion is
/// `RH = −6 + 125 · S_RH / 65535`.
fn rh_pct(raw: u16) -> f32 {
    -6.0 + 125.0 * f32::from(raw) / 65_535.0
}

/// Verify one CRC-8-protected 16-bit word, returning its big-endian value;
/// `None` when the trailing CRC-8 does not match.
fn word(frame: &[u8]) -> Option<u16> {
    if crc8(&frame[..2]) != frame[2] {
        return None;
    }
    Some(u16::from_be_bytes([frame[0], frame[1]]))
}

// -- Driver ----------------------------------------------------------------

/// Why a driver step failed — the source executor's fault vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Bus(BusError),
    /// A measurement frame failed its CRC-8 check.
    Crc,
}

/// Driver phases across engine beats.
#[derive(Clone, Debug)]
enum Phase {
    /// Bring-up leg 1: soft reset (settles by the next beat).
    Init,
    /// Bring-up leg 2: serial-number probe (CRC-checked).
    Probe,
    /// Ready to issue the next measurement command.
    Ready,
    /// Measurement command issued; collect the frame on the next beat.
    Collecting,
}

/// The SHT4x driver: the per-instance state of one declared source. No
/// heap, no locks; the engine lends its bus to exactly one executor per
/// beat.
#[derive(Clone, Debug)]
pub struct Sht4x {
    phase: Phase,
    serial: Option<u32>,
}

impl Sht4x {
    pub const fn new() -> Self {
        Self {
            phase: Phase::Init,
            serial: None,
        }
    }

    /// The probed serial number once bring-up completed (identifier from
    /// `0x89` — useful as a stable per-device marker in status payloads).
    pub fn serial(&self) -> Option<u32> {
        self.serial
    }

    /// Advance the flow by one beat's worth of bus work. `Ok(None)` means
    /// the flow is parked (a leg completed, or a conversion is in
    /// flight) — the next beat resumes it. Any error makes the executor
    /// re-enter bring-up on the next beat.
    pub fn measure<B: I2c>(&mut self, bus: &mut B) -> Result<Option<Measurement>, Error> {
        match self.phase {
            Phase::Init => {
                bus.write(I2C_ADDRESS, &[CMD_SOFT_RESET])
                    .map_err(Error::Bus)?;
                self.phase = Phase::Probe;
                Ok(None)
            }
            Phase::Probe => {
                bus.write(I2C_ADDRESS, &[CMD_SERIAL]).map_err(Error::Bus)?;
                let mut frame = [0u8; FRAME_LEN];
                bus.read(I2C_ADDRESS, &mut frame).map_err(Error::Bus)?;
                let serial_hi = word(&frame[..3]).ok_or(Error::Crc)?;
                let serial_lo = word(&frame[3..]).ok_or(Error::Crc)?;
                self.serial = Some((u32::from(serial_hi) << 16) | u32::from(serial_lo));
                self.phase = Phase::Ready;
                Ok(None)
            }
            Phase::Ready => {
                bus.write(I2C_ADDRESS, &[CMD_MEASURE_HIGH])
                    .map_err(Error::Bus)?;
                self.phase = Phase::Collecting;
                Ok(None)
            }
            Phase::Collecting => {
                let mut frame = [0u8; FRAME_LEN];
                bus.read(I2C_ADDRESS, &mut frame).map_err(Error::Bus)?;
                let raw_t = word(&frame[..3]).ok_or(Error::Crc)?;
                let raw_rh = word(&frame[3..]).ok_or(Error::Crc)?;
                self.phase = Phase::Ready;
                // Bounded integer inputs through the datasheet formulas:
                // the finite check cannot fire.
                Ok(Some(
                    Measurement::environmental(
                        celsius(raw_t),
                        rh_pct(raw_rh),
                        // Pressure is not a property of this part.
                        0.0,
                    )
                    .expect("datasheet-sourced conversion is finite"),
                ))
            }
        }
    }
}

impl Default for Sht4x {
    fn default() -> Self {
        Self::new()
    }
}

/// Register-flow scripts and canonical vectors shared with the engine's
/// register-flow tests — one copy per part, so the byte sequences the
/// driver tests pin and the ones the engine tests replay cannot drift.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::bus::SimulatedI2c;

    /// Script the bring-up legs (reset, serial probe) — the flow reaches
    /// Ready after two beats.
    pub(crate) fn script_init(bus: &mut SimulatedI2c) {
        bus.expect_write(I2C_ADDRESS, &[CMD_SOFT_RESET]);
        bus.expect_write(I2C_ADDRESS, &[CMD_SERIAL]);
        // Serial 0x12345678, CRCs computed with the datasheet's algorithm.
        bus.expect_read(I2C_ADDRESS, &[0x12, 0x34, 0x37, 0x56, 0x78, 0x7D]);
    }

    /// Script one measurement: command leg, then the collected frame —
    /// a raw frame that reads 25.0 °C / ~56.5 %RH.
    pub(crate) fn script_measurement(bus: &mut SimulatedI2c) {
        bus.expect_write(I2C_ADDRESS, &[CMD_MEASURE_HIGH]);
        bus.expect_read(I2C_ADDRESS, &[0x66, 0x66, 0x93, 0x80, 0x00, 0xA2]);
    }

    /// The measurement the scripted frame collects — the pin shared by
    /// the driver and engine tests.
    pub(crate) fn canonical() -> Measurement {
        Measurement::Environmental {
            temperature: 25.0,
            humidity: 56.500_954,
            pressure: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{canonical, script_init, script_measurement};
    use super::*;
    use crate::bus::SimulatedI2c;

    #[test]
    fn the_command_flow_produces_the_pinned_reading() {
        let mut bus = SimulatedI2c::new();
        script_init(&mut bus);
        script_measurement(&mut bus);

        let mut d = Sht4x::new();
        // Bring-up: two beats, no measurements.
        assert_eq!(d.measure(&mut bus), Ok(None)); // soft reset
        assert_eq!(d.measure(&mut bus), Ok(None)); // serial probe
        // Command leg, then collection leg.
        assert_eq!(d.measure(&mut bus), Ok(None));
        let m = d.measure(&mut bus).expect("collection succeeds");
        assert_eq!(m, Some(canonical()));
        // The driver issued exactly the scripted transactions, in order.
        assert_eq!(bus.logged(), 5);
        assert!(bus.script_served());
    }

    #[test]
    fn the_recorded_flow_is_command_then_read() {
        let mut bus = SimulatedI2c::new();
        script_init(&mut bus);
        script_measurement(&mut bus);
        let mut d = Sht4x::new();
        for _ in 0..4 {
            d.measure(&mut bus).unwrap();
        }

        use crate::bus::OpKind::*;
        // reset w | serial w | serial r | measure w | frame r
        let kinds = [Write, Write, Read, Write, Read];
        for (i, kind) in kinds.iter().enumerate() {
            let op = bus.op(i).unwrap();
            assert_eq!(op.kind, *kind, "op {i}");
            assert_eq!(op.addr, I2C_ADDRESS, "op {i}");
        }
        // The SHT4x has no register file: reads never follow a pointer
        // write on the same transaction (no write_read anywhere).
        for i in 0..bus.logged() {
            assert_ne!(bus.op(i).unwrap().kind, WriteRead);
        }
        assert_eq!(
            bus.op(2).unwrap().received(),
            &[0x12, 0x34, 0x37, 0x56, 0x78, 0x7D]
        );
        assert_eq!(
            bus.op(4).unwrap().received(),
            &[0x66, 0x66, 0x93, 0x80, 0x00, 0xA2]
        );
    }

    #[test]
    fn a_corrupted_frame_is_rejected_as_a_crc_fault() {
        let mut bus = SimulatedI2c::new();
        // Right bytes, broken CRC on the temperature word. The flow is
        // parked in Collecting by hand, so a single read is the whole op.
        bus.expect_read(I2C_ADDRESS, &[0x66, 0x66, 0x00, 0x80, 0x00, 0xA2]);

        let mut d = Sht4x::new();
        d.phase = Phase::Collecting;
        assert_eq!(d.measure(&mut bus), Err(Error::Crc));
        assert_eq!(bus.logged(), 1);
    }

    #[test]
    fn a_dead_bus_faults_without_emitting() {
        let mut bus = SimulatedI2c::new(); // nothing scripted
        let mut d = Sht4x::new();
        assert_eq!(d.measure(&mut bus), Err(Error::Bus(BusError::Timeout)));
        // And it keeps faulting (never panics, never blocks).
        assert_eq!(d.measure(&mut bus), Err(Error::Bus(BusError::Timeout)));
        assert_eq!(bus.logged(), 2);
    }

    #[test]
    fn crc8_matches_the_datasheet_worked_example() {
        assert_eq!(crc8(&[0xBE, 0xEF]), 0x92); // datasheet worked example
        // Checksums of the pinned frames' words.
        assert_eq!(crc8(&[0x66, 0x66]), 0x93);
        assert_eq!(crc8(&[0x80, 0x00]), 0xA2);
        assert_eq!(crc8(&[0x12, 0x34]), 0x37);
        assert_eq!(crc8(&[0x56, 0x78]), 0x7D);
    }

    #[test]
    fn conversions_reproduce_the_pinned_frame() {
        // The frame's raw words convert to 25.0 °C (exact in f32) and
        // −6 + 125·0x8000/65535 ≈ 56.5 %RH.
        assert_eq!(celsius(0x6666), 25.0);
        assert!((rh_pct(0x8000) - 56.5).abs() < 1e-3);
        // Conversion endpoints: raw 0 → −45.0 °C / −6 %, raw 0xFFFF →
        // +130.0 °C / +119 % — the formulas' verbatim domain.
        assert_eq!(celsius(0), -45.0);
        assert!((celsius(0xFFFF) - 130.0).abs() < 1e-3);
        assert_eq!(rh_pct(0), -6.0);
        assert!((rh_pct(0xFFFF) - 119.0).abs() < 1e-3);
    }

    #[test]
    fn the_serial_number_is_probed_crc_checked_and_kept() {
        let mut bus = SimulatedI2c::new();
        script_init(&mut bus);
        let mut d = Sht4x::new();
        d.measure(&mut bus).unwrap();
        d.measure(&mut bus).unwrap();
        assert_eq!(d.serial(), Some(0x1234_5678));
        assert!(bus.script_served());
    }
}
