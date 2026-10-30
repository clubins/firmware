//! SHT4x source node — gated by the `sensor-sht4x` flag (pulls
//! `driver-i2c`).
//!
//! Driver contract (spec §Sensor Expansion): I²C; temperature, humidity →
//! `weather` measurements. Pressure is not a property of this part (the
//! node leaves it unset).
//!
//! The flow follows the SHT4x datasheet: unlike the BME280's register
//! file, the SHT4x is command-driven — the host issues a command byte,
//! waits out the conversion, then clocks out a fixed frame. Every 16-bit
//! word in a frame is followed by a Sensirion CRC-8 (poly 0x31, init
//! 0xFF; the datasheet's worked example maps `0xBEEF` to `0x92`), and a
//! frame with a bad checksum is rejected rather than converted.
//!
//! Poll shape: each poll performs one leg of the flow — bring-up polls
//! issue the soft reset and then the serial-number probe (a CRC-checked
//! frame; the natural next poll arrives long after the 1 ms reset time),
//! a Ready poll issues the high-precision measurement command (`0xFD`,
//! ≤ 8.3 ms conversion), and the following poll collects and converts the
//! 6-byte frame. No spinning, no timers: the pump cadence paces the
//! datasheet's waits.
//!
//! Fixed-point output: temperature 0.1 °C (`i16`, clamped to the part's
//! −40..125 °C operating range), humidity whole % (clamped 0..100, per
//! the datasheet's conversion formulas). Vectors are pinned in the tests,
//! including the conversion endpoints.

use crate::bus::{BusError, I2c};
use crate::measurement::Measurement;
use crate::nodekit::{self, MeasRing};

/// I²C address (all SHT4x variants; no address strap on this part).
pub const I2C_ADDRESS: u8 = 0x44;

/// Ingest type name on the wire (HTTP path `/v1/sensors/weather`, MQTT
/// topic suffix `…/data/weather`).
pub const INGEST_TYPE: &str = "weather";

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

/// Raw ticks → 0.1 °C. The datasheet's conversion is
/// `T = −45 + 175 · S_T / 65535` °C; evaluated directly in tenths with a
/// single rounding, then clamped to the part's operating range.
fn t10(raw: u16) -> i16 {
    let t = -450 + ((1_750 * u32::from(raw) + 32_767) / 65_535) as i32;
    t.clamp(-400, 1_250) as i16
}

/// Raw ticks → whole %RH. The datasheet's conversion is
/// `RH = −6 + 125 · S_RH / 65535` %, clamped to 0..100 (per the
/// datasheet, raw 0 underflows and 0xFFFF overflows by design).
fn pct(raw: u16) -> u8 {
    let rh = -6 + ((125 * u32::from(raw) + 32_767) / 65_535) as i32;
    rh.clamp(0, 100) as u8
}

/// Verify one CRC-8-protected 16-bit word, returning its big-endian value.
fn word(frame: &[u8]) -> Result<u16, BusError> {
    if crc8(&frame[..2]) != frame[2] {
        return Err(BusError::Nack);
    }
    Ok(u16::from_be_bytes([frame[0], frame[1]]))
}

// -- Driver ----------------------------------------------------------------

/// Why a driver step failed — the node's fault counter vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Bus(BusError),
    /// A measurement frame failed its CRC-8 check.
    Crc,
}

/// Driver phases across polls.
enum Phase {
    /// Bring-up leg 1: soft reset (settles by the next poll).
    Init,
    /// Bring-up leg 2: serial-number probe (CRC-checked).
    Probe,
    /// Ready to issue the next measurement command.
    Ready,
    /// Measurement command issued; collect the frame on the next poll.
    Collecting,
}

/// The SHT4x driver: one instance per device, statically allocated in its
/// node. No heap, no locks; the bus is the pump's exclusively.
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

    /// Advance the flow by one poll's worth of bus work. `Ok(None)` means
    /// the flow is parked (a leg completed, or a conversion is in
    /// flight) — the next poll resumes it. Any error re-enters bring-up.
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
                let serial_hi = word(&frame[..3]).map_err(|_| Error::Crc)?;
                let serial_lo = word(&frame[3..]).map_err(|_| Error::Crc)?;
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
                let raw_t = word(&frame[..3]).map_err(|_| Error::Crc)?;
                let raw_rh = word(&frame[3..]).map_err(|_| Error::Crc)?;
                self.phase = Phase::Ready;
                Ok(Some(Measurement::Weather {
                    temperature_c: Some(t10(raw_t)),
                    humidity_pct: Some(pct(raw_rh)),
                    pressure_hpa: None,
                }))
            }
        }
    }
}

impl Default for Sht4x {
    fn default() -> Self {
        Self::new()
    }
}

// -- Graph node --------------------------------------------------------------

/// The SHT4x node: polled I²C in, measurements out.
pub struct Node {
    /// Completed measurements waiting for a transport drain.
    pub out: MeasRing,
    driver: Sht4x,
    readings: u64,
    emitted: u64,
    faults: u64,
}

impl Default for Node {
    fn default() -> Self {
        Self::new()
    }
}

impl Node {
    pub const fn new() -> Self {
        Self {
            out: MeasRing::new(),
            driver: Sht4x::new(),
            readings: 0,
            emitted: 0,
            faults: 0,
        }
    }

    /// Polls (bus legs attempted) since boot.
    pub fn readings(&self) -> u64 {
        self.readings
    }

    /// Measurements accepted into the output ring since boot.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Faulted polls since boot (bus errors, CRC failures).
    pub fn faults(&self) -> u64 {
        self.faults
    }

    /// Static footprint of this node (state + ring), for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step over the bus: one flow leg — bring-up, command
    /// issue, or frame collection. Returns 1 when a measurement was
    /// emitted.
    pub fn poll<B: I2c>(&mut self, bus: &mut B) -> usize {
        self.readings = self.readings.wrapping_add(1);
        match self.driver.measure(bus) {
            Ok(Some(m)) => {
                if nodekit::offer(&self.out, m) {
                    self.emitted += 1;
                    1
                } else {
                    0
                }
            }
            Ok(None) => 0,
            Err(_) => {
                // Any fault re-enters bring-up: the flow heals itself from
                // the next poll on.
                self.driver = Sht4x::new();
                self.faults += 1;
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The datasheet conversion worked in reverse: a raw frame that reads
    /// 25.0 °C / 57 %RH, CRCs computed with the datasheet's algorithm.
    const FRAME6: [u8; 6] = [0x66, 0x66, 0x93, 0x80, 0x00, 0xA2];

    /// Script the three bring-up legs (reset, serial probe) — the flow
    /// reaches Ready after two polls.
    fn script_init(bus: &mut crate::bus::SimulatedI2c) {
        bus.expect_write(I2C_ADDRESS, &[CMD_SOFT_RESET]);
        bus.expect_write(I2C_ADDRESS, &[CMD_SERIAL]);
        bus.expect_read(I2C_ADDRESS, &[0x12, 0x34, 0x37, 0x56, 0x78, 0x7D]); // serial 0x12345678
    }

    /// Script one measurement: command leg, then the collected frame.
    fn script_measurement(bus: &mut crate::bus::SimulatedI2c) {
        bus.expect_write(I2C_ADDRESS, &[CMD_MEASURE_HIGH]);
        bus.expect_read(I2C_ADDRESS, &FRAME6);
    }

    #[test]
    fn the_command_flow_produces_the_pinned_reading() {
        let mut bus = crate::bus::SimulatedI2c::new();
        script_init(&mut bus);
        script_measurement(&mut bus);

        let mut node = Node::new();
        // Bring-up: two polls, no measurements.
        assert_eq!(node.poll(&mut bus), 0); // soft reset
        assert_eq!(node.poll(&mut bus), 0); // serial probe
        assert_eq!(node.faults(), 0);
        // Command leg, then collection leg.
        assert_eq!(node.poll(&mut bus), 0);
        assert_eq!(node.poll(&mut bus), 1);
        assert_eq!(node.readings(), 4);
        assert_eq!(node.emitted(), 1);
        assert_eq!(
            node.out.pop(),
            Some(Measurement::Weather {
                temperature_c: Some(250), // 25.0 °C
                humidity_pct: Some(57),
                pressure_hpa: None,
            })
        );
        // The driver issued exactly the scripted transactions, in order.
        assert_eq!(bus.logged(), 5);
        assert!(bus.script_served());
    }

    #[test]
    fn the_recorded_flow_is_command_then_read() {
        let mut bus = crate::bus::SimulatedI2c::new();
        script_init(&mut bus);
        script_measurement(&mut bus);
        let mut node = Node::new();
        for _ in 0..4 {
            node.poll(&mut bus);
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
        assert_eq!(bus.op(4).unwrap().received(), &FRAME6);
    }

    #[test]
    fn a_corrupted_frame_faults_and_bring_up_repeats() {
        let mut bus = crate::bus::SimulatedI2c::new();
        script_init(&mut bus);
        // First measurement frame: right bytes, broken CRC.
        bus.expect_write(I2C_ADDRESS, &[CMD_MEASURE_HIGH]);
        bus.expect_read(I2C_ADDRESS, &[0x66, 0x66, 0x00, 0x80, 0x00, 0xA2]);
        // The fault re-enters bring-up: script the full recovery flow,
        // then the clean measurement.
        bus.expect_write(I2C_ADDRESS, &[CMD_SOFT_RESET]);
        bus.expect_write(I2C_ADDRESS, &[CMD_SERIAL]);
        bus.expect_read(I2C_ADDRESS, &[0x12, 0x34, 0x37, 0x56, 0x78, 0x7D]);
        script_measurement(&mut bus);

        let mut node = Node::new();
        node.poll(&mut bus); // reset
        node.poll(&mut bus); // serial
        assert_eq!(node.poll(&mut bus), 0); // command issued
        assert_eq!(node.poll(&mut bus), 0); // CRC failure — faulted
        assert_eq!(node.faults(), 1);
        assert_eq!(node.emitted(), 0);
        // The node re-enters bring-up and still delivers the next reading.
        node.poll(&mut bus); // reset
        node.poll(&mut bus); // serial
        node.poll(&mut bus); // command
        assert_eq!(node.poll(&mut bus), 1);
        assert_eq!(node.faults(), 1);
        assert_eq!(node.emitted(), 1);
        assert!(bus.script_served());
    }

    #[test]
    fn a_dead_bus_faults_without_emitting() {
        let mut bus = crate::bus::SimulatedI2c::new(); // nothing scripted
        let mut node = Node::new();
        assert_eq!(node.poll(&mut bus), 0);
        assert_eq!(node.faults(), 1);
        assert_eq!(node.emitted(), 0);
        // And it keeps faulting (never panics, never blocks).
        assert_eq!(node.poll(&mut bus), 0);
        assert_eq!(node.faults(), 2);
    }

    #[test]
    fn crc8_matches_the_datasheet_worked_examples() {
        assert_eq!(crc8(&[0xBE, 0xEF]), 0x92); // datasheet worked example
        // Checksums of the pinned frames' words.
        assert_eq!(crc8(&[0x66, 0x66]), 0x93);
        assert_eq!(crc8(&[0x80, 0x00]), 0xA2);
        assert_eq!(crc8(&[0x12, 0x34]), 0x37);
        assert_eq!(crc8(&[0x56, 0x78]), 0x7D);
    }

    #[test]
    fn conversions_reproduce_the_pinned_frame_and_endpoints() {
        // The frame's raw words convert to 25.0 °C / 57 %RH.
        assert_eq!(t10(0x6666), 250);
        assert_eq!(pct(0x8000), 57);
        // Endpoints of the conversion formulas, clamped to the part's
        // operating range: raw 0 → −45.0 → −40.0 °C; raw 0xFFFF → +130.0 →
        // +125.0 °C; humidity saturates at 0 and 100.
        assert_eq!(t10(0), -400);
        assert_eq!(t10(0xFFFF), 1_250);
        assert_eq!(pct(0), 0);
        assert_eq!(pct(0xFFFF), 100);
        // Inside the range the conversion is verbatim.
        assert_eq!(t10(0x8000), 425); // −45 + 87.5 = 42.5 °C
        assert_eq!(pct(0x4000), 25); // −6 + 31.25 → 25
    }

    #[test]
    fn the_serial_number_is_probed_crc_checked_and_kept() {
        let mut bus = crate::bus::SimulatedI2c::new();
        script_init(&mut bus);
        let mut node = Node::new();
        node.poll(&mut bus);
        node.poll(&mut bus);
        assert_eq!(node.driver.serial(), Some(0x1234_5678));
        assert!(bus.script_served());
    }
}
