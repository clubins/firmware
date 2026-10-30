//! BME280 source node — gated by the `sensor-bme280` flag (pulls
//! `driver-i2c`).
//!
//! Driver contract (spec §Sensor Expansion): I²C; temperature, humidity,
//! pressure → `weather` measurements. The server's weather fields also
//! include precipitation/rain-probability/wind, but those are virtual
//! Open-Meteo fields today — the node leaves them unset.
//!
//! The register flow follows the Bosch datasheet (BST-BME280-DS002):
//!
//! 1. identify (`0xD0` == `0x60`), soft-reset (`0xE0` = `0xB6`), wait for
//!    the NVM copy (`status.im_update`), burst-read the calibration words
//!    (`0x88..0xA1`, `0xE1..0xE7`), then configure (`0xF2` ctrl_hum first
//!    — it arms with the next ctrl_meas write — and `0xF5` config while
//!    still in sleep mode);
//! 2. per conversion: trigger forced mode (`0xF4`), read the status
//!    register (`0xF3`, `measuring` bit) once — the pump's step period
//!    dwarfs the 9.3 ms max conversion, so a busy status parks the flow
//!    until the next poll instead of spinning;
//! 3. burst-read the results (`0xF7..0xFE`) and compensate with the
//!    datasheet's integer formulas (64-bit pressure, 64-bit humidity
//!    variants; `t_fine` carries temperature into pressure and humidity).
//!
//! Arithmetic mirrors the datasheet's C code exactly (same shifts, same
//! constants, truncating division). Valid calibration cannot overflow; a
//! debug build panics on garbage calibration where C would wrap, and the
//! release build wraps like C.
//!
//! Fixed-point output: temperature 0.1 °C (`i16`, clamped to the chip's
//! −40..85 °C operating range), humidity whole % (clamped 0..100),
//! pressure whole hPa (clamped to `u16`). Vectors are pinned against the
//! datasheet's canonical worked example (T = 25.08 °C, p = 100 653 Pa)
//! in the tests.

use crate::bus::{BusError, I2c};
use crate::measurement::Measurement;
use crate::nodekit::{self, MeasRing};

/// Default I²C address (SDO tied low; `0x77` when high).
pub const I2C_ADDRESS: u8 = 0x76;

/// Ingest type name on the wire (HTTP path `/v1/sensors/weather`, MQTT
/// topic suffix `…/data/weather`).
pub const INGEST_TYPE: &str = "weather";

// -- Register map (BST-BME280-DS002 §5) -----------------------------------

const REG_ID: u8 = 0xD0;
const REG_RESET: u8 = 0xE0;
const REG_STATUS: u8 = 0xF3;
const REG_CTRL_HUM: u8 = 0xF2;
const REG_CTRL_MEAS: u8 = 0xF4;
const REG_CONFIG: u8 = 0xF5;
/// Burst start: pressure MSB first — P[3] | T[3] | H[2] = 8 bytes.
const REG_PRESS_MSB: u8 = 0xF7;
const REG_CALIB_T_P: u8 = 0x88; // 26 bytes: dig_T1..dig_P9, reserved, dig_H1
const REG_CALIB_H: u8 = 0xE1; // 7 bytes: dig_H2..dig_H6

const CHIP_ID: u8 = 0x60;
const SOFT_RESET_CMD: u8 = 0xB6;
/// `status.im_update` — NVM copy in progress (boot).
const STATUS_IM_UPDATE: u8 = 1 << 0;
/// `status.measuring` — conversion in progress.
const STATUS_MEASURING: u8 = 1 << 3;

/// Cap on status re-reads while the post-reset NVM copy completes
/// (datasheet: ≤ ~3 ms; 32 reads at the bus clock cover it with margin).
const NVM_COPY_MAX_POLLS: u32 = 32;

/// Oversampling ×1 for temperature, pressure and humidity — the fastest
/// configuration the part offers (≤ 9.3 ms), fitting any pump cadence.
const OSRS_H_X1: u8 = 0x01;
const CTRL_MEAS_FORCED_X1: u8 = 0x25; // osrs_t = 1 << 5 | osrs_p = 1 << 2 | forced
const CONFIG_SLEEP: u8 = 0x00; // filter off; standby is forced-mode-irrelevant

// -- Compensation (datasheet §8.2, integer formulas) -----------------------

/// The burst-read calibration words.
#[derive(Debug, Clone, Copy)]
struct Calibration {
    dig_t1: u16,
    dig_t2: i16,
    dig_t3: i16,
    dig_p1: u16,
    dig_p2: i16,
    dig_p3: i16,
    dig_p4: i16,
    dig_p5: i16,
    dig_p6: i16,
    dig_p7: i16,
    dig_p8: i16,
    dig_p9: i16,
    dig_h1: u8,
    dig_h2: i16,
    dig_h3: u8,
    dig_h4: i16,
    dig_h5: i16,
    dig_h6: i8,
}

impl Calibration {
    /// Parse the two calibration bursts: `0x88..0xA1` (26 bytes — T1..P9,
    /// one reserved byte, H1) and `0xE1..0xE7` (7 bytes).
    fn parse(block1: &[u8; 26], block2: &[u8; 7]) -> Self {
        let le = |b: &[u8], i: usize| u16::from(b[i]) | (u16::from(b[i + 1]) << 8);
        let se = |raw: u16| raw as i16;
        // dig_H4/H5 are signed 12-bit values packed around dig_H5's nibbles.
        let i12 = |raw: u16| {
            if raw & 0x0800 != 0 {
                (raw | 0xF000) as i16
            } else {
                raw as i16
            }
        };
        Self {
            dig_t1: le(block1, 0),
            dig_t2: se(le(block1, 2)),
            dig_t3: se(le(block1, 4)),
            dig_p1: le(block1, 6),
            dig_p2: se(le(block1, 8)),
            dig_p3: se(le(block1, 10)),
            dig_p4: se(le(block1, 12)),
            dig_p5: se(le(block1, 14)),
            dig_p6: se(le(block1, 16)),
            dig_p7: se(le(block1, 18)),
            dig_p8: se(le(block1, 20)),
            dig_p9: se(le(block1, 22)),
            dig_h1: block1[25],
            dig_h2: se(le(block2, 0)),
            dig_h3: block2[2],
            dig_h4: i12((u16::from(block2[3]) << 4) | u16::from(block2[4] & 0x0F)),
            dig_h5: i12((u16::from(block2[5]) << 4) | u16::from(block2[4] >> 4)),
            dig_h6: block2[6] as i8,
        }
    }
}

/// Temperature compensation: returns `(temperature in 0.01 °C, t_fine)`.
/// `t_fine` must be carried into the pressure and humidity calls.
fn compensate_t(cal: &Calibration, adc_t: i32) -> (i32, i32) {
    let var1 =
        (((adc_t >> 3) - (i32::from(cal.dig_t1) << 1)) * i32::from(cal.dig_t2)) >> 11;
    let x = (adc_t >> 4) - i32::from(cal.dig_t1);
    let var2 = (((x * x) >> 12) * i32::from(cal.dig_t3)) >> 14;
    let t_fine = var1 + var2;
    let t = (t_fine * 5 + 128) >> 8;
    (t, t_fine)
}

/// Pressure compensation in Pa as Q24.8 (datasheet `compensate_P_int64`:
/// an output of 25 767 233 represents 100 653.3 Pa). Returns 0 for a
/// degenerate calibration (Bosch's division guard).
fn compensate_p(cal: &Calibration, adc_p: i32, t_fine: i32) -> u32 {
    let var1 = i64::from(t_fine) - 128_000;
    let mut var2 = var1 * var1 * i64::from(cal.dig_p6);
    var2 += (var1 * i64::from(cal.dig_p5)) << 17;
    var2 += i64::from(cal.dig_p4) << 35;
    // Bosch evaluates both correction terms against the ORIGINAL var1 —
    // keep the combination in one expression so the update can't leak.
    let div = ((((var1 * var1 * i64::from(cal.dig_p3)) >> 8)
        + ((var1 * i64::from(cal.dig_p2)) << 12)
        + (1i64 << 47))
        * i64::from(cal.dig_p1))
        >> 33;
    if div == 0 {
        return 0;
    }
    let mut p = 1_048_576 - i64::from(adc_p);
    p = ((p << 31) - var2) * 3125 / div;
    let v1 = (i64::from(cal.dig_p9) * (p >> 13) * (p >> 13)) >> 25;
    let v2 = (i64::from(cal.dig_p8) * p) >> 19;
    let p = ((p + v1 + v2) >> 8) + (i64::from(cal.dig_p7) << 4);
    p as u32
}

/// Humidity compensation in %RH as Q22.10 (datasheet: an output of 47445
/// represents 46.333 %RH).
fn compensate_h(cal: &Calibration, adc_h: i32, t_fine: i32) -> u32 {
    let mut x1 = t_fine - 76_800;
    let lin = (((((x1 * i32::from(cal.dig_h6)) >> 10)
        * (((x1 * i32::from(cal.dig_h3)) >> 11) + 32_768))
        >> 10)
        + 2_097_152)
        * i32::from(cal.dig_h2)
        + 8_192;
    x1 = ((((adc_h << 14)
        - (i32::from(cal.dig_h4) << 20)
        - (i32::from(cal.dig_h5) * x1))
        + 16_384)
        >> 15)
        * (lin >> 14);
    x1 -= ((((x1 >> 15) * (x1 >> 15)) >> 7) * i32::from(cal.dig_h1)) >> 4;
    x1 = x1.clamp(0, 419_430_400);
    (x1 >> 12) as u32
}

/// 0.01 °C → 0.1 °C, clamped to the chip's operating range (−40..85 °C).
fn t10(centi: i32) -> i16 {
    ((centi + 5).div_euclid(10)).clamp(-400, 850) as i16
}

/// Q24.8 Pa → whole hPa, clamped to the wire type.
fn hpa(q24_8: u32) -> u16 {
    ((u64::from(q24_8) + 12_800) / 25_600).min(u16::MAX as u64) as u16
}

/// Q22.10 %RH → whole %, clamped to 0..100.
fn pct(q22_10: u32) -> u8 {
    (((u64::from(q22_10) + 512) / 1024).min(100)) as u8
}

// -- Driver ----------------------------------------------------------------

/// Why a driver step failed — the node's fault counter vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Bus(BusError),
    /// The id register returned another part's value (the byte received).
    WrongId(u8),
    /// The flow reached a conversion without calibration — an internal
    /// invariant violation (bring-up always precedes conversions).
    State,
}

/// Driver phases across polls.
enum Phase {
    /// Bring-up: identify, reset, calibrate, configure. Retried every poll
    /// until it succeeds — a faulted sensor stays this node's problem.
    Init,
    /// Ready to trigger the next forced conversion.
    Ready,
    /// Conversion triggered; parked until `status.measuring` reads clear.
    Converting,
}

/// The BME280 driver: one instance per device, statically allocated in its
/// node. No heap, no locks; the bus is the pump's exclusively.
pub struct Bme280 {
    phase: Phase,
    cal: Option<Calibration>,
}

impl Bme280 {
    pub const fn new() -> Self {
        Self {
            phase: Phase::Init,
            cal: None,
        }
    }

    /// Advance the flow by one poll's worth of bus work. `Ok(None)` means
    /// the flow is parked (bring-up, or a busy conversion) — the next poll
    /// resumes it. Any error re-enters bring-up on the next poll.
    pub fn measure<B: I2c>(&mut self, bus: &mut B) -> Result<Option<Measurement>, Error> {
        match self.phase {
            Phase::Init => {
                self.init(bus)?;
                Ok(None)
            }
            Phase::Ready => {
                // Trigger a forced conversion (the part returns to sleep
                // after it, so every conversion re-arms).
                bus.write(I2C_ADDRESS, &[REG_CTRL_MEAS, CTRL_MEAS_FORCED_X1])
                    .map_err(Error::Bus)?;
                if self.status_busy(bus)? {
                    self.phase = Phase::Converting;
                    return Ok(None);
                }
                self.read_results(bus).map(Some)
            }
            Phase::Converting => {
                if self.status_busy(bus)? {
                    return Ok(None);
                }
                self.read_results(bus).map(Some)
            }
        }
    }

    /// Bring-up, exactly as the datasheet sequences it. Leaves the part
    /// configured (sleep mode) with calibration in hand.
    fn init<B: I2c>(&mut self, bus: &mut B) -> Result<(), Error> {
        let mut id = [0u8; 1];
        bus.write_read(I2C_ADDRESS, &[REG_ID], &mut id)
            .map_err(Error::Bus)?;
        if id[0] != CHIP_ID {
            return Err(Error::WrongId(id[0]));
        }
        bus.write(I2C_ADDRESS, &[REG_RESET, SOFT_RESET_CMD])
            .map_err(Error::Bus)?;
        // The NVM copy after reset must finish before calibration reads
        // (datasheet: ≤ ~3 ms — 32 status reads at the bus clock cover it
        // with margin). Bounded: a stuck part faults the node instead of
        // spinning the pump forever.
        let mut status = [0u8; 1];
        for _ in 0..NVM_COPY_MAX_POLLS {
            bus.write_read(I2C_ADDRESS, &[REG_STATUS], &mut status)
                .map_err(Error::Bus)?;
            if status[0] & STATUS_IM_UPDATE == 0 {
                break;
            }
        }
        if status[0] & STATUS_IM_UPDATE != 0 {
            return Err(Error::Bus(BusError::Timeout));
        }

        let mut block1 = [0u8; 26];
        bus.write_read(I2C_ADDRESS, &[REG_CALIB_T_P], &mut block1)
            .map_err(Error::Bus)?;
        let mut block2 = [0u8; 7];
        bus.write_read(I2C_ADDRESS, &[REG_CALIB_H], &mut block2)
            .map_err(Error::Bus)?;
        self.cal = Some(Calibration::parse(&block1, &block2));

        // ctrl_hum arms with the next ctrl_meas write; config only applies
        // in sleep mode, which the reset just left us in.
        bus.write(I2C_ADDRESS, &[REG_CTRL_HUM, OSRS_H_X1])
            .map_err(Error::Bus)?;
        bus.write(I2C_ADDRESS, &[REG_CONFIG, CONFIG_SLEEP])
            .map_err(Error::Bus)?;
        self.phase = Phase::Ready;
        Ok(())
    }

    /// One status read: is a conversion still running?
    fn status_busy<B: I2c>(&mut self, bus: &mut B) -> Result<bool, Error> {
        let mut status = [0u8; 1];
        bus.write_read(I2C_ADDRESS, &[REG_STATUS], &mut status)
            .map_err(Error::Bus)?;
        Ok(status[0] & STATUS_MEASURING != 0)
    }

    /// Burst-read `0xF7..0xFE`, compensate, and hand back the weather
    /// measurement. Leaves the driver Ready for the next conversion.
    fn read_results<B: I2c>(&mut self, bus: &mut B) -> Result<Measurement, Error> {
        let Some(cal) = self.cal else {
            return Err(Error::State); // unreachable: bring-up precedes
        };
        let mut raw = [0u8; 8];
        bus.write_read(I2C_ADDRESS, &[REG_PRESS_MSB], &mut raw)
            .map_err(Error::Bus)?;
        self.phase = Phase::Ready;

        let adc_p = ((u32::from(raw[0]) << 12)
            | (u32::from(raw[1]) << 4)
            | u32::from(raw[2]) >> 4) as i32;
        let adc_t = ((u32::from(raw[3]) << 12)
            | (u32::from(raw[4]) << 4)
            | u32::from(raw[5]) >> 4) as i32;
        let adc_h = (u32::from(raw[6]) << 8) | u32::from(raw[7]);

        let (t_centi, t_fine) = compensate_t(&cal, adc_t);
        let p_q = compensate_p(&cal, adc_p, t_fine);
        let h_q = compensate_h(&cal, adc_h as i32, t_fine);

        Ok(Measurement::Weather {
            temperature_c: Some(t10(t_centi)),
            humidity_pct: Some(pct(h_q)),
            // 0 means the pressure path was skipped (Bosch's guard value).
            pressure_hpa: (p_q != 0).then(|| hpa(p_q)),
        })
    }
}

impl Default for Bme280 {
    fn default() -> Self {
        Self::new()
    }
}

// -- Graph node --------------------------------------------------------------

/// The BME280 node: polled I²C in, measurements out.
pub struct Node {
    /// Completed measurements waiting for a transport drain.
    pub out: MeasRing,
    driver: Bme280,
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
            driver: Bme280::new(),
            readings: 0,
            emitted: 0,
            faults: 0,
        }
    }

    /// Polls (bus flows attempted) since boot.
    pub fn readings(&self) -> u64 {
        self.readings
    }

    /// Measurements accepted into the output ring since boot.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Faulted polls since boot (bus errors, wrong id).
    pub fn faults(&self) -> u64 {
        self.faults
    }

    /// Static footprint of this node (state + ring), for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step over the bus: bring-up or one conversion, whichever
    /// the flow needs. Returns 1 when a measurement was emitted.
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
                self.driver = Bme280::new();
                self.faults += 1;
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Calibration block from the datasheet's canonical example
    // (BST-BME280-DS002 §4.2.2), as bytes: dig_T1..dig_P9 little-endian,
    // one reserved byte (0xA0), then dig_H1.
    const CAL26: [u8; 26] = [
        0x70, 0x6B, // T1 = 27504
        0x43, 0x67, // T2 = 26435
        0x18, 0xFC, // T3 = -1000
        0x7D, 0x8E, // P1 = 36477
        0x43, 0xD6, // P2 = -10685
        0xD0, 0x0B, // P3 = 3024
        0x27, 0x0B, // P4 = 2855
        0x8C, 0x00, // P5 = 140
        0xF9, 0xFF, // P6 = -7
        0x8C, 0x3C, // P7 = 15500
        0xF8, 0xC6, // P8 = -14600
        0x70, 0x17, // P9 = 6000
        0x00, // reserved (0xA0)
        0x4B, // H1 = 75
    ];
    const CAL7: [u8; 7] = [0x6B, 0x01, 0x00, 0x13, 0x05, 0x1E, 0x1E]; // H2 = 363, H4 = 309, H5 = 480, H6 = 30
    // The datasheet example's raw conversion result: T_raw = 519888,
    // P_raw = 415148, H_raw = 0x6FF0 (synthetic).
    const FRAME8: [u8; 8] = [0x65, 0x5A, 0xC0, 0x7E, 0xED, 0x00, 0x6F, 0xF0];

    /// Script the bring-up flow. After one poll the driver is Ready.
    fn script_init(bus: &mut crate::bus::SimulatedI2c) {
        bus.expect_write_read(I2C_ADDRESS, &[REG_ID], &[CHIP_ID]);
        bus.expect_write(I2C_ADDRESS, &[REG_RESET, SOFT_RESET_CMD]);
        bus.expect_write_read(I2C_ADDRESS, &[REG_STATUS], &[0x00]);
        bus.expect_write_read(I2C_ADDRESS, &[REG_CALIB_T_P], &CAL26);
        bus.expect_write_read(I2C_ADDRESS, &[REG_CALIB_H], &CAL7);
        bus.expect_write(I2C_ADDRESS, &[REG_CTRL_HUM, OSRS_H_X1]);
        bus.expect_write(I2C_ADDRESS, &[REG_CONFIG, CONFIG_SLEEP]);
    }

    /// Script one conversion: trigger, status clear, burst read.
    fn script_conversion(bus: &mut crate::bus::SimulatedI2c) {
        bus.expect_write(I2C_ADDRESS, &[REG_CTRL_MEAS, CTRL_MEAS_FORCED_X1]);
        bus.expect_write_read(I2C_ADDRESS, &[REG_STATUS], &[0x00]);
        bus.expect_write_read(I2C_ADDRESS, &[REG_PRESS_MSB], &FRAME8);
    }

    #[test]
    fn the_datasheet_flow_produces_the_canonical_reading() {
        let mut bus = crate::bus::SimulatedI2c::new();
        script_init(&mut bus);
        script_conversion(&mut bus);

        let mut node = Node::new();
        // Poll 1: bring-up (no measurement yet).
        assert_eq!(node.poll(&mut bus), 0);
        assert_eq!(node.faults(), 0);
        // Poll 2: trigger + status clear + read + compensate.
        assert_eq!(node.poll(&mut bus), 1);
        assert_eq!(node.readings(), 2);
        assert_eq!(node.emitted(), 1);
        assert_eq!(
            node.out.pop(),
            Some(Measurement::Weather {
                temperature_c: Some(251), // 25.08 °C → 25.1
                humidity_pct: Some(42),
                pressure_hpa: Some(1007), // 100 653.3 Pa
            })
        );
        // The driver issued exactly the scripted transactions, in order.
        assert_eq!(bus.logged(), 10);
        assert!(bus.script_served());
    }

    #[test]
    fn the_recorded_flow_matches_the_datasheet_sequence() {
        let mut bus = crate::bus::SimulatedI2c::new();
        script_init(&mut bus);
        script_conversion(&mut bus);
        let mut node = Node::new();
        node.poll(&mut bus);
        node.poll(&mut bus);

        use crate::bus::OpKind::*;
        for (i, kind) in [
            WriteRead, Write, WriteRead, WriteRead, WriteRead, Write, Write, Write,
            WriteRead, WriteRead,
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(bus.op(i).unwrap().kind, *kind, "op {i}");
            assert_eq!(bus.op(i).unwrap().addr, I2C_ADDRESS, "op {i}");
        }
        // Register pointers hit exactly the datasheet's registers, in order.
        let pointers: Vec<u8> = (0..bus.logged())
            .map(|i| bus.op(i).unwrap().written()[0])
            .collect();
        assert_eq!(
            pointers,
            [0xD0, 0xE0, 0xF3, 0x88, 0xE1, 0xF2, 0xF5, 0xF4, 0xF3, 0xF7]
        );
        // Reset command, control bytes and read lengths.
        assert_eq!(bus.op(1).unwrap().written(), &[0xE0, 0xB6]);
        assert_eq!(bus.op(7).unwrap().written(), &[0xF4, 0x25]);
        assert_eq!(bus.op(3).unwrap().received().len(), 26);
        assert_eq!(bus.op(9).unwrap().received().len(), 8);
    }

    #[test]
    fn a_busy_status_parks_the_flow_until_the_next_poll() {
        let mut bus = crate::bus::SimulatedI2c::new();
        script_init(&mut bus);
        bus.expect_write(I2C_ADDRESS, &[REG_CTRL_MEAS, CTRL_MEAS_FORCED_X1]);
        bus.expect_write_read(I2C_ADDRESS, &[REG_STATUS], &[0x08]); // measuring
        // The conversion completes on the next poll's status check.
        bus.expect_write_read(I2C_ADDRESS, &[REG_STATUS], &[0x00]);
        bus.expect_write_read(I2C_ADDRESS, &[REG_PRESS_MSB], &FRAME8);

        let mut node = Node::new();
        node.poll(&mut bus); // init
        assert_eq!(node.poll(&mut bus), 0); // conversion busy — parked
        assert_eq!(node.emitted(), 0);
        assert_eq!(node.poll(&mut bus), 1); // resumed and emitted
        assert_eq!(node.emitted(), 1);
        assert_eq!(bus.logged(), 11);
        assert!(bus.script_served());
    }

    #[test]
    fn a_wrong_chip_id_faults_and_retries_bring_up() {
        let mut bus = crate::bus::SimulatedI2c::new();
        bus.expect_write_read(I2C_ADDRESS, &[REG_ID], &[0x00]); // not a BME280
        script_init(&mut bus); // …so the next poll's bring-up is scripted

        let mut node = Node::new();
        assert_eq!(node.poll(&mut bus), 0);
        assert_eq!(node.faults(), 1);
        assert_eq!(node.readings(), 1);
        // The next poll re-runs the whole bring-up.
        assert_eq!(node.poll(&mut bus), 0);
        assert_eq!(node.faults(), 1);
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
    fn a_stuck_nvm_copy_faults_instead_of_spinning() {
        // The part answers every status read with im_update still set.
        // The script cap (32) bounds how long the bus can play along:
        // id, reset, then 30 busy statuses — bring-up must terminate and
        // fault the node rather than spin the pump forever, whatever
        // ends the wait (cap or bus timeout).
        let mut bus = crate::bus::SimulatedI2c::new();
        bus.expect_write_read(I2C_ADDRESS, &[REG_ID], &[CHIP_ID]);
        bus.expect_write(I2C_ADDRESS, &[REG_RESET, SOFT_RESET_CMD]);
        for _ in 0..30 {
            bus.expect_write_read(I2C_ADDRESS, &[REG_STATUS], &[STATUS_IM_UPDATE]);
        }

        let mut node = Node::new();
        assert_eq!(node.poll(&mut bus), 0);
        assert_eq!(node.faults(), 1);
        assert_eq!(node.emitted(), 0);
        // The entire script was consumed or attempted (32 recorded).
        assert_eq!(bus.logged(), 32);
        // And the next poll re-enters bring-up without hanging.
        assert_eq!(node.poll(&mut bus), 0);
        assert_eq!(node.faults(), 2);
    }

    #[test]
    fn compensation_matches_the_datasheet_worked_example() {
        let cal = Calibration::parse(&CAL26, &CAL7);
        let (t_centi, t_fine) = compensate_t(&cal, 519_888);
        // Datasheet canonical example: adc_T = 519888 → 25.08 °C.
        assert_eq!(t_centi, 2508);
        assert_eq!(t_fine, 128_422);
        // adc_P = 415148 → 100 653.3 Pa.
        let p = compensate_p(&cal, 415_148, t_fine);
        assert_eq!(p, 25_767_233);
        assert_eq!(hpa(p), 1007);
        // adc_H = 0x6FF0 with this calibration → 41.5 %RH.
        let h = compensate_h(&cal, 0x6FF0, t_fine);
        assert_eq!(pct(h), 42);
    }

    #[test]
    fn fixed_point_output_clamps_to_the_measurement_type() {
        // Temperature below the operating range clamps at −40.0 °C.
        assert_eq!(t10(-4_500), -400);
        // Above it clamps at 85.0 °C.
        assert_eq!(t10(9_000), 850);
        // Humidity saturates below 0 and above 100.
        assert_eq!(pct(0), 0);
        assert_eq!(pct(419_430_400), 100);
        // Pressure clamps to the u16 wire type instead of truncating.
        assert_eq!(hpa(u32::MAX), u16::MAX);
    }

    #[test]
    fn the_calibration_parser_sign_extends_the_12_bit_words() {
        // H4 = -35 packs as E4 = 0xFD, E5 low nibble = 0xD; H5 = -12 as
        // E6 = 0xFF, E5 high nibble = 0x4.
        let block2 = [0x6B, 0x01, 0x00, 0xFD, 0x4D, 0xFF, 0x1E];
        let cal = Calibration::parse(&CAL26, &block2);
        assert_eq!(cal.dig_h4, -35);
        assert_eq!(cal.dig_h5, -12);
        assert_eq!(cal.dig_h2, 363);
        assert_eq!(cal.dig_h6, 30);
    }
}
