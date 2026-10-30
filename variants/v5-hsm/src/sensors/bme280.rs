//! BME280 driver — gated by the `sensor-bme280` flag.
//!
//! I2C temperature, humidity, and pressure → the `weather` ingest type,
//! emitted as the firmware-core [`EnvironmentalReading`].
//!
//! The register flow is the datasheet's forced-mode cycle (Bosch
//! BST-BME280-DS002; compensation transcribed from the Bosch reference
//! driver `BME280_driver` v3.5.1, double-precision paths): read the chip
//! id (0xD0, expect 0x60), soft-reset on mismatch (0xE0 ← 0xB6, then wait
//! the 2 ms startup time), burst-read the calibration (26 bytes from 0x88,
//! 7 from 0xE1), write `ctrl_hum` (0xF2) and `config` (0xF5), then per
//! conversion: trigger 0xF4 in forced mode, wait the datasheet's maximum
//! measurement time, poll `status` (0xF3) until the `measuring` bit
//! clears, and burst-read the 8 data bytes at 0xF7. Compensation runs in
//! f64 like the reference, clamps to the datasheet ranges (−40..85 °C,
//! 300..1100 hPa, 0..100 %RH), and casts to f32 at the edge.
//!
//! Pacing is event-driven — the machine never blocks: the converting state
//! waits for `SIG_TICK` past the computed deadline instead of delaying.
//! The stub machine at the bottom of this file remains the
//! composition-root placeholder until the esp-hal I2C adapter lands
//! (docs/plan/build-dag.md, `feat/driver-buses`); [`driver`] builds the
//! real machine over a bus and clock, host-tested against recorded
//! transactions (`ScriptedBus`).

use crate::events::{Event, SIG_ENGAGE, SIG_MEAS_READY, SIG_TICK, Signal};
use crate::fsm::{Ctx, Outcome, StateDef, StateId};
use crate::measurement::EnvironmentalReading;
use crate::sensors::bus::{Clock, I2cBus};
use crate::stubkit::{self, HsmMachine, StubCtx};
use alloc::boxed::Box;

/// Driver-domain signal: a forced-mode conversion is due. The cadence
/// posts it; this machine subscribes.
pub const SIG_BME280_CONVERT: Signal = Signal::new(120);

/// Dispatcher position: sensors deliver before transports and ops.
pub const PRIORITY: u8 = 12;

/// Signals this machine wants: bring-up, the conversion cadence, and the
/// shared tick for datasheet-timed waits.
pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_BME280_CONVERT, SIG_TICK];

// Registers and timing (Bosch BST-BME280-DS002).
const ADDR: u8 = 0x76; // 7-bit address with SDO low — the board's default
const REG_CHIP_ID: u8 = 0xD0; // reads 0x60 on a BME280
const CHIP_ID: u8 = 0x60;
const REG_RESET: u8 = 0xE0; // soft reset: write 0xB6
const RESET_CMD: u8 = 0xB6;
const REG_STATUS: u8 = 0xF3; // bit 3: measuring
const STATUS_MEASURING: u8 = 0x08;
const REG_CTRL_HUM: u8 = 0xF2;
const REG_CTRL_MEAS: u8 = 0xF4;
const REG_CONFIG: u8 = 0xF5;
const REG_DATA: u8 = 0xF7; // press[19:0], temp[19:0], hum[15:0], 8 bytes
const CALIB_TP_LEN: usize = 26; // 0x88..=0xA1: dig_T1..dig_P9, reserved, dig_H1
const CALIB_H_LEN: usize = 7; // 0xE1..=0xE7: dig_H2..dig_H6
const DATA_LEN: usize = 8;
/// Reset → NVM-copy startup time (datasheet Table 1: 2 ms).
const RESET_STARTUP_MS: u64 = 2;
const MODE_FORCED: u8 = 0b01;

/// Oversampling setting (datasheet §3.3.1); [`Oversampling::Skip`] disables
/// a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Oversampling {
    Skip,
    X1,
    X2,
    X4,
    X8,
    X16,
}

impl Oversampling {
    /// Register encoding for the `osrs_*` fields of ctrl_hum / ctrl_meas.
    fn bits(self) -> u8 {
        match self {
            Self::Skip => 0b000,
            Self::X1 => 0b001,
            Self::X2 => 0b010,
            Self::X4 => 0b011,
            Self::X8 => 0b100,
            Self::X16 => 0b101,
        }
    }

    /// Conversions per measurement, for the duration model.
    fn multiplier(self) -> u32 {
        match self {
            Self::Skip => 0,
            Self::X1 => 1,
            Self::X2 => 2,
            Self::X4 => 4,
            Self::X8 => 8,
            Self::X16 => 16,
        }
    }
}

/// This driver's operating point: ×1 on every channel — the datasheet's
/// lowest-noise-per-power tradeoff at weather-station cadence.
const OSR_T: Oversampling = Oversampling::X1;
const OSR_P: Oversampling = Oversampling::X1;
const OSR_H: Oversampling = Oversampling::X1;

/// Maximum forced-mode measurement duration in whole ms (datasheet §8.3:
/// 1.25 + 2.3·osr_t + (2.3·osr_p + 0.575) + (2.3·osr_h + 0.575), rounded
/// up). The converting state waits this long before the first status poll.
pub fn max_measurement_ms(
    osr_t: Oversampling,
    osr_p: Oversampling,
    osr_h: Oversampling,
) -> u64 {
    const MEAS_OFFSET_MS: f64 = 1.25;
    const MEAS_DUR_MS: f64 = 2.3;
    const PRES_HUM_OFFSET_MS: f64 = 0.575;
    let total = MEAS_OFFSET_MS
        + MEAS_DUR_MS * osr_t.multiplier() as f64
        + MEAS_DUR_MS * osr_p.multiplier() as f64
        + PRES_HUM_OFFSET_MS
        + MEAS_DUR_MS * osr_h.multiplier() as f64
        + PRES_HUM_OFFSET_MS;
    // ceil, by hand: the crate is no_std and `total` is always positive
    // (≥ 9.3 ms), where this expansion of `f64::ceil` is exact.
    let whole = total as u64;
    if total > whole as f64 {
        whole + 1
    } else {
        whole
    }
}

/// The ctrl_meas byte this driver writes per conversion: forced mode at
/// the configured oversampling.
fn trigger_byte() -> u8 {
    OSR_T.bits() << 5 | OSR_P.bits() << 2 | MODE_FORCED
}

/// Calibration coefficients, decoded from the sensor's NVM (datasheet
/// §3.11.2 with the reference driver's signed reassembly of H4/H5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calibration {
    pub dig_t1: u16,
    pub dig_t2: i16,
    pub dig_t3: i16,
    pub dig_p1: u16,
    pub dig_p2: i16,
    pub dig_p3: i16,
    pub dig_p4: i16,
    pub dig_p5: i16,
    pub dig_p6: i16,
    pub dig_p7: i16,
    pub dig_p8: i16,
    pub dig_p9: i16,
    pub dig_h1: u8,
    pub dig_h2: i16,
    pub dig_h3: u8,
    pub dig_h4: i16,
    pub dig_h5: i16,
    pub dig_h6: i8,
}

impl Calibration {
    /// Decodes the two calibration bursts: 26 bytes from 0x88 (dig_T1 through
    /// dig_P9 little-endian, one reserved byte, dig_H1) and 7 from 0xE1.
    pub fn from_bursts(tp: &[u8; CALIB_TP_LEN], h: &[u8; CALIB_H_LEN]) -> Self {
        let u16le = |lo: u8, hi: u8| u16::from(lo) | u16::from(hi) << 8;
        let i16le = |lo: u8, hi: u8| u16le(lo, hi) as i16;
        Self {
            dig_t1: u16le(tp[0], tp[1]),
            dig_t2: i16le(tp[2], tp[3]),
            dig_t3: i16le(tp[4], tp[5]),
            dig_p1: u16le(tp[6], tp[7]),
            dig_p2: i16le(tp[8], tp[9]),
            dig_p3: i16le(tp[10], tp[11]),
            dig_p4: i16le(tp[12], tp[13]),
            dig_p5: i16le(tp[14], tp[15]),
            dig_p6: i16le(tp[16], tp[17]),
            dig_p7: i16le(tp[18], tp[19]),
            dig_p8: i16le(tp[20], tp[21]),
            dig_p9: i16le(tp[22], tp[23]),
            dig_h1: tp[25],
            dig_h2: i16le(h[0], h[1]),
            dig_h3: h[2],
            dig_h4: i16::from(h[3] as i8) * 16 + i16::from(h[4] & 0x0F),
            dig_h5: i16::from(h[5] as i8) * 16 + i16::from((h[4] >> 4) & 0x0F),
            dig_h6: h[6] as i8,
        }
    }
}

/// Compensated temperature in °C plus the fine residue (`t_fine`, int32)
/// the pressure and humidity formulas share — both from one double sum,
/// exactly as the reference driver derives them.
pub fn temperature_and_fine(calib: &Calibration, adc_t: u32) -> (f32, i32) {
    let var1 = (f64::from(adc_t) / 16_384.0 - f64::from(calib.dig_t1) / 1024.0)
        * f64::from(calib.dig_t2);
    let var2 = (f64::from(adc_t) / 131_072.0 - f64::from(calib.dig_t1) / 8_192.0)
        * (f64::from(adc_t) / 131_072.0 - f64::from(calib.dig_t1) / 8_192.0)
        * f64::from(calib.dig_t3);
    let t_fine = (var1 + var2) as i32;
    let temperature = ((var1 + var2) / 5120.0).clamp(-40.0, 85.0) as f32;
    (temperature, t_fine)
}

/// Compensated pressure in Pa (reference `compensate_pressure`, double
/// path), clamped to the sensor's 300..1100 hPa range.
pub fn pressure_pa(calib: &Calibration, adc_p: u32, t_fine: i32) -> f32 {
    let t_fine = f64::from(t_fine);
    let mut var1 = t_fine / 2.0 - 64_000.0;
    let mut var2 = var1 * var1 * f64::from(calib.dig_p6) / 32_768.0;
    var2 += var1 * f64::from(calib.dig_p5) * 2.0;
    var2 = var2 / 4.0 + f64::from(calib.dig_p4) * 65_536.0;
    let var3 = f64::from(calib.dig_p3) * var1 * var1 / 524_288.0;
    var1 = (var3 + f64::from(calib.dig_p2) * var1) / 524_288.0;
    var1 = (1.0 + var1 / 32_768.0) * f64::from(calib.dig_p1);
    let pressure = if var1 > 0.0 {
        let p = (1_048_576.0 - f64::from(adc_p) - var2 / 4_096.0) * 6_250.0 / var1;
        let var1 = f64::from(calib.dig_p9) * p * p / 2_147_483_648.0;
        let var2 = p * f64::from(calib.dig_p8) / 32_768.0;
        p + (var1 + var2 + f64::from(calib.dig_p7)) / 16.0
    } else {
        30_000.0
    };
    pressure.clamp(30_000.0, 110_000.0) as f32
}

/// Compensated relative humidity in %RH (reference
/// `bme280_calculate_humidity`, double path), clamped to 0..100.
pub fn humidity_percent(calib: &Calibration, adc_h: u16, t_fine: i32) -> f32 {
    let var1 = f64::from(t_fine) - 76_800.0;
    let var2 =
        f64::from(calib.dig_h4) * 64.0 + (f64::from(calib.dig_h5) / 16_384.0) * var1;
    let var3 = f64::from(adc_h) - var2;
    let var4 = f64::from(calib.dig_h2) / 65_536.0;
    let var5 = (1.0 + (f64::from(calib.dig_h6) / 67_108_864.0) * var1)
        * (1.0 + (f64::from(calib.dig_h3) / 537_395_200.0) * var1);
    let var7 =
        var3 * var4 * ((1.0 - (f64::from(calib.dig_h3) / 537_395_200.0) * var1) / var5);
    let humidity = var7 * (1.0 - f64::from(calib.dig_h1) * var7 / 524_288.0);
    humidity.clamp(0.0, 100.0) as f32
}

/// Compensates one raw ADC triplet into the environmental reading:
/// temperature first (pressure and humidity both consume its `t_fine`).
pub fn compensate(
    calib: &Calibration,
    adc_p: u32,
    adc_t: u32,
    adc_h: u16,
) -> EnvironmentalReading {
    let (temperature_c, t_fine) = temperature_and_fine(calib, adc_t);
    EnvironmentalReading {
        temperature_c,
        humidity_percent: humidity_percent(calib, adc_h, t_fine),
        pressure_hpa: pressure_pa(calib, adc_p, t_fine) / 100.0,
    }
}

// ---- the driver machine ----

pub const ROOT: StateId = 0;
pub const ONLINE: StateId = 1;
pub const PROBING: StateId = 2;
pub const CALIBRATING: StateId = 3;
pub const READY: StateId = 4;
pub const CONVERTING: StateId = 5;
pub const FAULT: StateId = 6;

/// What the last entry action concluded — handlers consult it on the
/// next tick instead of re-reading the bus.
#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Probed,
    Resetting,
    Calibrated,
    Faulted,
}

/// The BME280 machine's context: the bus and clock it runs over plus the
/// observable state the system (and tests) read.
pub struct Driver {
    bus: Box<dyn I2cBus>,
    clock: Box<dyn Clock>,
    /// Decoded calibration once bring-up has read it.
    pub calib: Option<Calibration>,
    /// The most recent compensated reading; set when `SIG_MEAS_READY` posts.
    pub reading: Option<EnvironmentalReading>,
    /// Completed conversions since registration.
    pub measurements: u32,
    /// Failed transactions (bring-up or conversion) since registration.
    pub faults: u32,
    /// The sensor's serial-bus address.
    pub addr: u8,
    phase: Phase,
    abort: bool,
    /// Earliest ms at which the pending wait (reset startup / conversion)
    /// may conclude — the event-driven stand-in for `delay_ms`.
    resume_at: u64,
}

fn root_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Transit(PROBING),
        _ => Outcome::Unhandled,
    }
}

fn online_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        // Already brought up; a repeated engage is idempotent.
        s if s == SIG_ENGAGE => Outcome::Handled,
        _ => Outcome::Unhandled,
    }
}

/// Bring-up step 1: read the chip id. A mismatch writes the soft reset
/// and the machine waits out the startup time on ticks — an external
/// self-transition re-runs this entry, which is the re-probe.
fn probing_entry(d: &mut Driver, _: &mut Ctx) {
    let mut id = [0u8; 1];
    d.phase = match d.bus.write_read(d.addr, &[REG_CHIP_ID], &mut id) {
        Ok(()) if id[0] == CHIP_ID => Phase::Probed,
        Ok(()) => {
            if d.bus.write(d.addr, &[REG_RESET, RESET_CMD]).is_ok() {
                d.resume_at = d.clock.now_ms() + RESET_STARTUP_MS;
                Phase::Resetting
            } else {
                d.faults += 1;
                Phase::Faulted
            }
        }
        Err(_) => {
            d.faults += 1;
            Phase::Faulted
        }
    };
}

/// Bring-up runs in the entry actions; this handler only files the last
/// outcome when the next tick arrives — initial transitions may only
/// descend, so a probe result cannot hop sideways on `INIT`.
fn probing_handler(d: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled, // mid-probe engage is a no-op
        s if s == SIG_TICK => match d.phase {
            Phase::Probed => Outcome::Transit(CALIBRATING),
            Phase::Resetting if d.clock.now_ms() >= d.resume_at => {
                // Startup time elapsed: the external self-transition
                // re-runs the entry action — the re-probe.
                Outcome::Transit(PROBING)
            }
            Phase::Resetting => Outcome::Unhandled, // still within 2 ms startup
            _ => Outcome::Transit(FAULT),
        },
        _ => Outcome::Unhandled,
    }
}

/// Bring-up step 2: read both calibration bursts, then set ctrl_hum and
/// config before any ctrl_meas write can trigger a conversion.
fn calibrating_entry(d: &mut Driver, _: &mut Ctx) {
    let mut tp = [0u8; CALIB_TP_LEN];
    let mut h = [0u8; CALIB_H_LEN];
    let ok = d.bus.write_read(d.addr, &[0x88], &mut tp).is_ok()
        && d.bus.write_read(d.addr, &[0xE1], &mut h).is_ok()
        && d.bus.write(d.addr, &[REG_CTRL_HUM, OSR_H.bits()]).is_ok()
        && d.bus.write(d.addr, &[REG_CONFIG, 0x00]).is_ok();
    if ok {
        d.calib = Some(Calibration::from_bursts(&tp, &h));
        d.phase = Phase::Calibrated;
    } else {
        d.faults += 1;
        d.phase = Phase::Faulted;
    }
}

fn calibrating_handler(d: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled, // mid-calibration engage is a no-op
        s if s == SIG_TICK => match d.phase {
            Phase::Calibrated => Outcome::Transit(READY),
            _ => Outcome::Transit(FAULT),
        },
        _ => Outcome::Unhandled,
    }
}

fn ready_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_BME280_CONVERT => Outcome::Transit(CONVERTING),
        _ => Outcome::Unhandled,
    }
}

/// One forced-mode conversion: trigger, wait the datasheet's maximum
/// measurement time, poll status, then burst-read and compensate.
fn converting_entry(d: &mut Driver, _: &mut Ctx) {
    d.abort = false;
    if d.bus
        .write(d.addr, &[REG_CTRL_MEAS, trigger_byte()])
        .is_ok()
    {
        d.resume_at = d.clock.now_ms() + max_measurement_ms(OSR_T, OSR_P, OSR_H);
    } else {
        // Conversion failures are retryable in place — skip to READY and
        // let the next cadence post try again.
        d.faults += 1;
        d.abort = true;
    }
}

fn converting_handler(d: &mut Driver, out: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_TICK => {
            if d.abort {
                // The trigger write failed — file the retry on this tick.
                return Outcome::Transit(READY);
            }
            if d.clock.now_ms() < d.resume_at {
                return Outcome::Unhandled; // conversion still within its window
            }
            let mut status = [0u8; 1];
            match d.bus.write_read(d.addr, &[REG_STATUS], &mut status) {
                Ok(()) if status[0] & STATUS_MEASURING != 0 => Outcome::Unhandled,
                Ok(()) => match read_and_compensate(d) {
                    Some(()) => {
                        out.post(Event::new(SIG_MEAS_READY));
                        Outcome::Transit(READY)
                    }
                    None => Outcome::Transit(READY),
                },
                Err(_) => {
                    d.faults += 1;
                    Outcome::Transit(READY)
                }
            }
        }
        // Busy: repeated cadence posts and re-engages are idempotent.
        s if s == SIG_BME280_CONVERT || s == SIG_ENGAGE => Outcome::Handled,
        _ => Outcome::Unhandled,
    }
}

/// Reads the 8 data bytes and compensates them. Returns `None` when the
/// reading is dropped (bus error or missing calibration — both counted
/// in `faults`).
fn read_and_compensate(d: &mut Driver) -> Option<()> {
    let mut data = [0u8; DATA_LEN];
    if d.bus.write_read(d.addr, &[REG_DATA], &mut data).is_err() {
        d.faults += 1;
        return None;
    }
    let Some(calib) = d.calib else {
        d.faults += 1;
        return None;
    };
    let adc_p =
        (u32::from(data[0]) << 12) | (u32::from(data[1]) << 4) | u32::from(data[2] >> 4);
    let adc_t =
        (u32::from(data[3]) << 12) | (u32::from(data[4]) << 4) | u32::from(data[5] >> 4);
    let adc_h = (u16::from(data[6]) << 8) | u16::from(data[7]);
    d.reading = Some(compensate(&calib, adc_p, adc_t, adc_h));
    d.measurements += 1;
    Some(())
}

fn fault_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_ENGAGE => Outcome::Transit(PROBING),
        _ => Outcome::Unhandled,
    }
}

static BME280_TABLE: &[StateDef<Driver>] = &[
    StateDef {
        name: "bme280:root",
        parent: None,
        entry: None,
        exit: None,
        handler: root_handler,
        transitions: &[],
    },
    StateDef {
        name: "bme280:online",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: online_handler,
        transitions: &[],
    },
    StateDef {
        name: "bme280:probing",
        parent: Some(ONLINE),
        entry: Some(probing_entry),
        exit: None,
        handler: probing_handler,
        transitions: &[],
    },
    StateDef {
        name: "bme280:calibrating",
        parent: Some(ONLINE),
        entry: Some(calibrating_entry),
        exit: None,
        handler: calibrating_handler,
        transitions: &[],
    },
    StateDef {
        name: "bme280:ready",
        parent: Some(ONLINE),
        entry: None,
        exit: None,
        handler: ready_handler,
        transitions: &[],
    },
    StateDef {
        name: "bme280:converting",
        parent: Some(ONLINE),
        entry: Some(converting_entry),
        exit: None,
        handler: converting_handler,
        transitions: &[],
    },
    StateDef {
        name: "bme280:fault",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: fault_handler,
        transitions: &[],
    },
];

/// Builds the real BME280 machine over a bus/clock pair. The target-side
/// composition root wires the esp-hal adapter here once it lands;
/// host tests pass `ScriptedBus` and `FixedClock`.
pub fn driver(bus: Box<dyn I2cBus>, clock: Box<dyn Clock>) -> HsmMachine<Driver> {
    let data = Driver {
        bus,
        clock,
        calib: None,
        reading: None,
        measurements: 0,
        faults: 0,
        addr: ADDR,
        phase: Phase::Probed,
        abort: false,
        resume_at: 0,
    };
    HsmMachine::new(
        "bme280",
        PRIORITY,
        SUBSCRIPTIONS,
        BME280_TABLE,
        ROOT,
        stubkit::STUB_DEFER_CAP,
        data,
    )
}

// ---- composition-root placeholder ----

/// The stub lifecycle machine the composition root registers until the
/// esp-hal I2C adapter exists (see the module doc). Deliberately shared
/// shape: wiring compiles unchanged when the driver takes over.
pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("bme280", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::sensors::bus::{BusError, SharedBus, SharedClock, Step};

    /// The datasheet's example calibration (Bosch BST-BME280-DS002 §8.2:
    /// dig_T1..dig_P3 and the example ADC triplet are verbatim; humidity
    /// coefficients are synthesized and cross-checked against the
    /// reference driver's double and int64 paths).
    fn example_tp_burst() -> [u8; CALIB_TP_LEN] {
        [
            0x70, 0x6B, // dig_T1 = 27504
            0x43, 0x67, // dig_T2 = 26435
            0x18, 0xFC, // dig_T3 = -1000
            0x7D, 0x8E, // dig_P1 = 36477
            0x43, 0xD6, // dig_P2 = -10685
            0xD0, 0x0B, // dig_P3 = 3024
            0x13, 0x1F, // dig_P4 = 7955
            0xEA, 0xF9, // dig_P5 = -1558
            0x18, 0x2E, // dig_P6 = 11800
            0x25, 0xDE, // dig_P7 = -8667
            0xCD, 0x4A, // dig_P8 = 19149
            0x4A, 0xFA, // dig_P9 = -1462
            0x00, // reserved (0xA0)
            0x41, // dig_H1 = 65
        ]
    }

    fn example_h_burst() -> [u8; CALIB_H_LEN] {
        [
            0x6A, 0x2C, // dig_H2 = 11370
            0x01, // dig_H3 = 1
            0x19, 0x58, // dig_H4 = 408, dig_H5 low nibble = 5
            0x21, // dig_H5 msb = 33 → dig_H5 = 533
            0x25, // dig_H6 = 37
        ]
    }

    fn calib() -> Calibration {
        Calibration::from_bursts(&example_tp_burst(), &example_h_burst())
    }

    #[test]
    fn parses_the_example_calibration() {
        let c = calib();
        assert_eq!(c.dig_t1, 27504);
        assert_eq!(c.dig_t2, 26435);
        assert_eq!(c.dig_t3, -1000);
        assert_eq!(c.dig_p1, 36477);
        assert_eq!(c.dig_p2, -10685);
        assert_eq!(c.dig_p9, -1462);
        assert_eq!(c.dig_h1, 65);
        assert_eq!(c.dig_h2, 11370);
        assert_eq!(c.dig_h3, 1);
        assert_eq!(c.dig_h4, 408);
        assert_eq!(c.dig_h5, 533);
        assert_eq!(c.dig_h6, 37);
    }

    #[test]
    fn compensates_the_datasheet_example() {
        // adc_P = 415148, adc_T = 519888 (datasheet §8.2 example), adc_H
        // synthesized. Expected values computed with the Bosch reference
        // driver's double paths: 25.0825 °C, 89048.2231 Pa, 51.8865 %RH
        // (the last on the synthesized humidity calibration).
        let reading = compensate(&calib(), 415_148, 519_888, 28_101);
        assert!(
            (reading.temperature_c - 25.0825).abs() < 0.001,
            "{:?}",
            reading
        );
        assert!(
            (reading.pressure_hpa - 890.4822).abs() < 0.01,
            "{:?}",
            reading
        );
        assert!(
            (reading.humidity_percent - 51.8865).abs() < 0.01,
            "{:?}",
            reading
        );
    }

    #[test]
    fn compensation_clamps_to_datasheet_ranges() {
        let c = calib();
        let (_, t_fine) = temperature_and_fine(&c, 519_888);
        // Impossible raw values clamp instead of leaking nonsense.
        let (t_lo, _) = temperature_and_fine(&c, 0);
        let (t_hi, _) = temperature_and_fine(&c, u32::MAX);
        assert!((-40.0..=85.0).contains(&t_lo));
        assert!((-40.0..=85.0).contains(&t_hi));
        assert!(pressure_pa(&c, 0, t_fine) >= 30_000.0);
        assert!(pressure_pa(&c, u32::from(u16::MAX) << 4 | 0xF, t_fine) <= 110_000.0);
        assert!(humidity_percent(&c, 0, t_fine) >= 0.0);
        assert!(humidity_percent(&c, u16::MAX, t_fine) <= 100.0);
    }

    #[test]
    fn timing_matches_the_datasheet_duration_model() {
        // ×1/×1/×1: 1.25 + 2.3 + 2.875 + 2.875 = 9.3 ms → 10.
        assert_eq!(
            max_measurement_ms(Oversampling::X1, Oversampling::X1, Oversampling::X1),
            10
        );
        // ×16 across the board: 1.25 + 36.8 + 37.375 + 37.375 = 112.8 → 113.
        assert_eq!(
            max_measurement_ms(Oversampling::X16, Oversampling::X16, Oversampling::X16),
            113
        );
        // The trigger byte: osrs_t=001, osrs_p=001, mode=01 → 0x25.
        assert_eq!(trigger_byte(), 0x25);
    }

    /// Bring-up script for the happy path: probe, calib bursts, config.
    fn bringup_script() -> Vec<Step> {
        vec![
            Step::WriteRead(ADDR, vec![REG_CHIP_ID], vec![CHIP_ID]),
            Step::WriteRead(ADDR, vec![0x88], example_tp_burst().into()),
            Step::WriteRead(ADDR, vec![0xE1], example_h_burst().into()),
            Step::Write(ADDR, vec![REG_CTRL_HUM, OSR_H.bits()]),
            Step::Write(ADDR, vec![REG_CONFIG, 0x00]),
        ]
    }

    const ADC_P: u32 = 415_148;
    const ADC_T: u32 = 519_888;
    const ADC_H: u16 = 28_101;

    fn data_burst() -> Vec<u8> {
        vec![
            ((ADC_P >> 12) & 0xFF) as u8,
            ((ADC_P >> 4) & 0xFF) as u8,
            ((ADC_P << 4) & 0xFF) as u8,
            ((ADC_T >> 12) & 0xFF) as u8,
            ((ADC_T >> 4) & 0xFF) as u8,
            ((ADC_T << 4) & 0xFF) as u8,
            (ADC_H >> 8) as u8,
            (ADC_H & 0xFF) as u8,
        ]
    }

    #[test]
    fn bring_up_reads_calibration_then_sits_ready() {
        let bus = SharedBus::new(bringup_script());
        let mut m = driver(Box::new(bus.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        // Probe ran in the entry action; the next two ticks file the probe
        // outcome (→ calibration) and the calibration outcome (→ ready).
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(m.data.calib.is_some());
        assert_eq!(m.data.faults, 0);
        bus.expect_done();
    }

    #[test]
    fn conversion_follows_the_recorded_flow() {
        let bus = SharedBus::new(
            bringup_script()
                .into_iter()
                .chain([
                    // Trigger 0xF4 = 0x25 (forced, ×1/×1/×1)…
                    Step::Write(ADDR, vec![REG_CTRL_MEAS, 0x25]),
                    // …status says done…
                    Step::WriteRead(ADDR, vec![REG_STATUS], vec![0x00]),
                    // …8-byte data burst at 0xF7.
                    Step::WriteRead(ADDR, vec![REG_DATA], data_burst()),
                ])
                .collect::<Vec<_>>(),
        );
        let clock = SharedClock::new(0);
        let mut m = driver(Box::new(bus.clone()), Box::new(clock.clone()));
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_BME280_CONVERT), &mut out);
        assert_eq!(m.hsm.current(), CONVERTING);
        // Datasheet max measurement time elapses, then the tick polls.
        clock.advance(10);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        let reading = m.data.reading.expect("compensated reading");
        assert!((reading.temperature_c - 25.0825).abs() < 0.001);
        assert!((reading.pressure_hpa - 890.4822).abs() < 0.01);
        assert!((reading.humidity_percent - 51.8865).abs() < 0.01);
        assert_eq!(m.data.measurements, 1);
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].signal, SIG_MEAS_READY);
        bus.expect_done();
    }

    #[test]
    fn a_wrong_chip_id_soft_resets_and_retries() {
        let clock = SharedClock::new(0);
        let bus = SharedBus::new(
            [
                Step::WriteRead(ADDR, vec![REG_CHIP_ID], vec![0x58]), // wrong id
                Step::Write(ADDR, vec![REG_RESET, RESET_CMD]),
                Step::WriteRead(ADDR, vec![REG_CHIP_ID], vec![CHIP_ID]),
            ]
            .into_iter()
            .chain(bringup_script().into_iter().skip(1))
            .collect::<Vec<_>>(),
        );
        let mut m = driver(Box::new(bus.clone()), Box::new(clock.clone()));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        // Wrong chip id: the reset command went out in the entry action and
        // the machine holds in PROBING through the 2 ms startup time.
        assert_eq!(m.hsm.current(), PROBING);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), PROBING);
        clock.advance(2);
        // Due: the external self-transition re-runs the probe entry…
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), PROBING);
        // …and the healthy re-probe files through calibration to ready.
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.faults, 0);
        bus.expect_done();
    }

    #[test]
    fn a_failed_probe_parks_in_fault_until_reengaged() {
        let bus = SharedBus::new(
            [Step::Fail(BusError::Nack)]
                .into_iter()
                .chain(bringup_script())
                .collect::<Vec<_>>(),
        );
        let mut m = driver(Box::new(bus.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), FAULT);
        assert_eq!(m.data.faults, 1);
        // Re-engage retries bring-up from the top.
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.faults, 1);
        bus.expect_done();
    }

    #[test]
    fn a_conversion_bus_error_drops_the_measurement_but_stays_up() {
        let bus = SharedBus::new(
            bringup_script()
                .into_iter()
                .chain([
                    Step::Write(ADDR, vec![REG_CTRL_MEAS, 0x25]),
                    Step::WriteRead(ADDR, vec![REG_STATUS], vec![0x00]),
                    Step::Fail(BusError::Nack), // data burst fails
                ])
                .collect::<Vec<_>>(),
        );
        let clock = SharedClock::new(0);
        let mut m = driver(Box::new(bus.clone()), Box::new(clock.clone()));
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_BME280_CONVERT), &mut out);
        clock.advance(10);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(m.data.reading.is_none());
        assert_eq!(m.data.measurements, 0);
        assert_eq!(m.data.faults, 1);
        assert!(posted.is_empty(), "no SIG_MEAS_READY without a reading");
        bus.expect_done();
    }

    #[test]
    fn a_still_measuring_status_defers_the_data_read_to_the_next_tick() {
        let bus = SharedBus::new(
            bringup_script()
                .into_iter()
                .chain([
                    Step::Write(ADDR, vec![REG_CTRL_MEAS, 0x25]),
                    Step::WriteRead(ADDR, vec![REG_STATUS], vec![STATUS_MEASURING]),
                    Step::WriteRead(ADDR, vec![REG_STATUS], vec![0x00]),
                    Step::WriteRead(ADDR, vec![REG_DATA], data_burst()),
                ])
                .collect::<Vec<_>>(),
        );
        let clock = SharedClock::new(0);
        let mut m = driver(Box::new(bus.clone()), Box::new(clock.clone()));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_BME280_CONVERT), &mut out);
        clock.advance(10);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), CONVERTING, "must keep waiting");
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.measurements, 1);
        bus.expect_done();
    }
}
