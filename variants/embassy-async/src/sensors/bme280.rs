//! BME280 — Bosch temperature / humidity / pressure over I²C
//! (`sensor-bme280` → `weather`).
//!
//! Register map and compensation follow Bosch's own BME280_driver reference
//! (`BoschSensortec/BME280_driver`, `bme280.c` — the datasheet's
//! double-precision compensation, BST-BME280-DS002 §8.2), transcribed from
//! that source in this session. The driver runs the sensor in **forced
//! mode**: each `sample` triggers one conversion (all three channels,
//! oversampling ×1), waits out the worst-case conversion time, then bursts
//! the eight data registers — one conversion per call, which is exactly one
//! event per `SensorDriver::sample`.
//!
//! Wire flow, per step:
//!
//! ```text
//! init:   read 0xD0 (chip id 0x60) → read 26 B @0x88 (T/P calib + H1)
//!         → read 7 B @0xE1 (H2..H6) → arm ctrl_hum (0xF2) → arm ctrl_meas
//! sample: write ctrl_meas = forced → wait conversion → burst 8 B @0xF7
//! ```
//!
//! `ctrl_hum` is written before `ctrl_meas` (datasheet: osrs_h only takes
//! effect on the next `ctrl_meas` write). The `config` register (IIR filter,
//! standby) stays at reset defaults: the filter is off and standby time is
//! meaningless in forced mode.
//!
//! Error codes (`SensorError.code`, this driver's causes):
//!
//! | code | fault | meaning |
//! |------|-------|---------|
//! | 1 | Bus | chip-id read failed |
//! | 2 | Hardware | chip id is not 0x60 — the device at this address is not a BME280 |
//! | 3 | Bus | calibration read failed |
//! | 4 | Bus | arming (`ctrl_hum`/`ctrl_meas`) failed |
//! | 5 | Bus | conversion trigger failed |
//! | 6 | Bus | data read failed |
//! | 7 | Reading | invalid raw markers (0x80000/0x8000) — no valid measurement |

use crate::events::{MeasurementPayload, WeatherSample};
use crate::sensor::{SensorDriver, SensorError, SensorFault};
use crate::sensors::Wait;
use crate::sensors::i2c::{BusError, I2cBus};

// Register addresses (BME280_driver `bme280_defs.h`).
const REG_CHIP_ID: u8 = 0xD0;
const REG_TEMP_PRESS_CALIB: u8 = 0x88;
const REG_HUM_CALIB: u8 = 0xE1;
const REG_CTRL_HUM: u8 = 0xF2;
const REG_CTRL_MEAS: u8 = 0xF4;
const REG_DATA: u8 = 0xF7;

/// Chip id answering at 0xD0 (`BME280_CHIP_ID`).
const CHIP_ID: u8 = 0x60;

// Oversampling ×1 for all three channels: 16.3 Pa RMS noise and ~1 °C /
// ~2 % RH precision — venue-weather granularity, at the fastest conversion.
const OSRS_X1: u8 = 0b001;

/// `ctrl_meas` with everything armed and the sensor in sleep mode (mode
/// bits 0b00): the bring-up state. Each sample rewrites it with forced mode.
const CTRL_MEAS_ARMED_SLEEP: u8 = OSRS_X1 << 5 | OSRS_X1 << 2;

/// `ctrl_meas` with forced mode (0b01): triggers one conversion.
const CTRL_MEAS_FORCED: u8 = CTRL_MEAS_ARMED_SLEEP | 0b01;

/// Worst-case forced-mode conversion time at ×1 oversampling, with margin:
/// `1.25 + 2·2.3 + 2·(2.3+0.575) + 2·(2.3+0.575)` ≈ 17.4 ms (datasheet
/// §9, measurement time table).
const CONVERSION_WAIT_MS: u64 = 20;

// Error codes — see the module table.
const ERR_CHIP_ID_READ: u8 = 1;
const ERR_CHIP_ID_MISMATCH: u8 = 2;
const ERR_CALIB_READ: u8 = 3;
const ERR_ARM_WRITE: u8 = 4;
const ERR_TRIGGER_WRITE: u8 = 5;
const ERR_DATA_READ: u8 = 6;
const ERR_INVALID_RAW: u8 = 7;

/// Raw calibration words, parsed little-endian per Bosch's
/// `parse_temp_press_calib_data` / `parse_humidity_calib_data`.
#[derive(Debug, Clone, Copy)]
struct Calibration {
    t1: u16,
    t2: i16,
    t3: i16,
    p1: u16,
    p2: i16,
    p3: i16,
    p4: i16,
    p5: i16,
    p6: i16,
    p7: i16,
    p8: i16,
    p9: i16,
    h1: u8,
    h2: i16,
    h3: u8,
    h4: i16,
    h5: i16,
    h6: i8,
}

impl Calibration {
    /// Parse the 26-byte block at 0x88 (`dig_T1..dig_P9` little-endian,
    /// reserved byte, `dig_H1`) and the 7-byte block at 0xE1 (`dig_H2..`).
    /// `dig_H4`/`dig_H5` are signed 12-bit values sharing the nibble byte —
    /// Bosch's sign-extension trick: the MSB byte is sign-extended, then
    /// shifted, then the nibble is OR-ed in.
    fn parse(tp: &[u8; 26], h: &[u8; 7]) -> Self {
        let u16le = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
        let i16le = |b: &[u8]| i16::from_le_bytes([b[0], b[1]]);
        Self {
            t1: u16le(&tp[0..2]),
            t2: i16le(&tp[2..4]),
            t3: i16le(&tp[4..6]),
            p1: u16le(&tp[6..8]),
            p2: i16le(&tp[8..10]),
            p3: i16le(&tp[10..12]),
            p4: i16le(&tp[12..14]),
            p5: i16le(&tp[14..16]),
            p6: i16le(&tp[16..18]),
            p7: i16le(&tp[18..20]),
            p8: i16le(&tp[20..22]),
            p9: i16le(&tp[22..24]),
            h1: tp[25],
            h2: i16le(&h[0..2]),
            h3: h[2],
            h4: ((h[3] as i8 as i16) << 4) | i16::from(h[4] & 0x0F),
            h5: ((h[5] as i8 as i16) << 4) | i16::from(h[4] >> 4),
            h6: h[6] as i8,
        }
    }
}

/// The BME280 driver: bus handle + wait seam + the calibration read at init.
pub struct Bme280<B, W> {
    bus: B,
    waiter: W,
    address: u8,
    /// `None` until `init` reads the calibration blocks. The framework's
    /// step ordering (init → calibrate → sample) is the only caller; the
    /// `Option` keeps a misuse from turning into a plausible zero reading.
    calib: Option<Calibration>,
}

impl<B, W> Bme280<B, W> {
    /// Build a driver for the sensor at `address` (0x76 with SDO low, 0x77
    /// high) over `bus`, waiting out conversions with `waiter`.
    #[must_use]
    pub const fn new(bus: B, waiter: W, address: u8) -> Self {
        Self {
            bus,
            waiter,
            address,
            calib: None,
        }
    }

    fn bus_fault(error: BusError, code: u8) -> SensorError {
        // The controller's vocabulary folds into the framework's: every bus
        // failure is a Bus fault; the code says which operation failed.
        let _ = error;
        SensorError::new(SensorFault::Bus, code)
    }
}

impl<B: I2cBus, W: Wait> SensorDriver for Bme280<B, W> {
    async fn init(&mut self) -> Result<(), SensorError> {
        let mut id = [0u8; 1];
        self.bus
            .write_read(self.address, &[REG_CHIP_ID], &mut id)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_CHIP_ID_READ))?;
        if id[0] != CHIP_ID {
            return Err(SensorError::new(
                SensorFault::Hardware,
                ERR_CHIP_ID_MISMATCH,
            ));
        }

        let mut tp = [0u8; 26];
        self.bus
            .write_read(self.address, &[REG_TEMP_PRESS_CALIB], &mut tp)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_CALIB_READ))?;
        let mut h = [0u8; 7];
        self.bus
            .write_read(self.address, &[REG_HUM_CALIB], &mut h)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_CALIB_READ))?;
        self.calib = Some(Calibration::parse(&tp, &h));

        // Arm the oversampling. ctrl_hum must be written first (osrs_h
        // latches on the next ctrl_meas write); sleep mode keeps the sensor
        // idle between samples.
        self.bus
            .write(self.address, &[REG_CTRL_HUM, OSRS_X1])
            .await
            .map_err(|error| Self::bus_fault(error, ERR_ARM_WRITE))?;
        self.bus
            .write(self.address, &[REG_CTRL_MEAS, CTRL_MEAS_ARMED_SLEEP])
            .await
            .map_err(|error| Self::bus_fault(error, ERR_ARM_WRITE))?;
        Ok(())
    }

    async fn calibrate(&mut self) -> Result<(), SensorError> {
        // The BME280 ships factory-trimmed (the calibration words read at
        // init are the unit's trim); this firmware adds no per-unit step.
        // The framework's per-unit calibration hook stays an Ok no-op.
        Ok(())
    }

    async fn sample(&mut self) -> Result<MeasurementPayload, SensorError> {
        let Some(calib) = self.calib else {
            // Unreachable through `SensorTask::step` (init precedes sample);
            // a misuse marker, never a zero reading.
            return Err(SensorError::new(SensorFault::Reading, 0));
        };

        self.bus
            .write(self.address, &[REG_CTRL_MEAS, CTRL_MEAS_FORCED])
            .await
            .map_err(|error| Self::bus_fault(error, ERR_TRIGGER_WRITE))?;
        self.waiter.wait(CONVERSION_WAIT_MS).await;

        let mut data = [0u8; 8];
        self.bus
            .write_read(self.address, &[REG_DATA], &mut data)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_DATA_READ))?;

        // 20-bit pressure and temperature, 16-bit humidity (Bosch's
        // `parse_sensor_data`).
        let adc_p =
            u32::from(data[0]) << 12 | u32::from(data[1]) << 4 | u32::from(data[2] >> 4);
        let adc_t =
            u32::from(data[3]) << 12 | u32::from(data[4]) << 4 | u32::from(data[5] >> 4);
        let adc_h = u16::from_be_bytes([data[6], data[7]]);

        // The datasheet marks "no valid measurement" with full-bit-set
        // registers (0x80000 for the 20-bit channels, 0x8000 for humidity):
        // a skipped conversion reads as these sentinels, never as zero.
        if adc_t == 0x80000 || adc_p == 0x80000 || adc_h == 0x8000 {
            return Err(SensorError::new(SensorFault::Reading, ERR_INVALID_RAW));
        }

        let (temperature, t_fine) = compensate_temperature(adc_t, &calib);
        let pressure = compensate_pressure(adc_p, &calib, t_fine);
        let humidity = compensate_humidity(adc_h, &calib, t_fine);

        Ok(MeasurementPayload::Weather(WeatherSample {
            temperature: Some(temperature),
            humidity: Some(humidity),
            // Rain, rain probability and wind are virtual Open-Meteo fields
            // server-side; no physical sensor in this firmware measures them.
            precipitation: None,
            rain_probability: None,
            wind_speed: None,
            pressure: Some(pressure),
        }))
    }
}

// ---------------------------------------------------------------------------
// Compensation — Bosch BME280_driver double-precision variant, ported from
// `bme280.c` (`compensate_temperature` / `_pressure` / `_humidity`). f64 in,
// Bosch's exact arithmetic, clamped to the datasheet ranges; the payload
// fields are f32, so the values narrow at the boundary.
// ---------------------------------------------------------------------------

type Compensated = (f32, f64);

/// Temperature from `adc_t`, returning (°C, `t_fine`) — `t_fine` feeds the
/// pressure and humidity compensation. Clamped to [-40, 85] °C.
fn compensate_temperature(adc_t: u32, c: &Calibration) -> Compensated {
    let var1 = (adc_t as f64 / 16_384.0 - f64::from(c.t1) / 1_024.0) * f64::from(c.t2);
    let mut var2 = adc_t as f64 / 131_072.0 - f64::from(c.t1) / 8_192.0;
    var2 *= var2;
    var2 *= f64::from(c.t3);
    let t_fine = var1 + var2;
    let temperature = (t_fine / 5_120.0).clamp(-40.0, 85.0);
    (temperature as f32, t_fine)
}

/// Pressure from `adc_p` in Pa, clamped to [30000, 110000] Pa; the
/// `var1 <= 0` degenerate calibration case reports the 30000 floor.
fn compensate_pressure(adc_p: u32, c: &Calibration, t_fine: f64) -> f32 {
    let mut var1 = t_fine / 2.0 - 64_000.0;
    let mut var2 = var1 * var1 * f64::from(c.p6) / 32_768.0;
    var2 += var1 * f64::from(c.p5) * 2.0;
    var2 = var2 / 4.0 + f64::from(c.p4) * 65_536.0;
    let var3 = f64::from(c.p3) * var1 * var1 / 524_288.0;
    var1 = (var3 + f64::from(c.p2) * var1) / 524_288.0;
    var1 = (1.0 + var1 / 32_768.0) * f64::from(c.p1);

    let pressure = if var1 > 0.0 {
        let mut pressure = 1_048_576.0 - f64::from(adc_p);
        pressure = (pressure - var2 / 4_096.0) * 6_250.0 / var1;
        let var1 = f64::from(c.p9) * pressure * pressure / 2_147_483_648.0;
        let var2 = pressure * f64::from(c.p8) / 32_768.0;
        pressure += (var1 + var2 + f64::from(c.p7)) / 16.0;
        pressure.clamp(30_000.0, 110_000.0)
    } else {
        30_000.0
    };
    pressure as f32
}

/// Relative humidity in %, clamped to [0, 100].
fn compensate_humidity(adc_h: u16, c: &Calibration, t_fine: f64) -> f32 {
    let var1 = t_fine - 76_800.0;
    let var2 = f64::from(c.h4) * 64.0 + (f64::from(c.h5) / 16_384.0) * var1;
    let var3 = f64::from(adc_h) - var2;
    let var4 = f64::from(c.h2) / 65_536.0;
    let var5 = 1.0 + (f64::from(c.h3) / 67_108_864.0) * var1;
    let mut var6 = 1.0 + (f64::from(c.h6) / 67_108_864.0) * var1 * var5;
    var6 = var3 * var4 * (var5 * var6);
    let humidity = var6 * (1.0 - f64::from(c.h1) * var6 / 524_288.0);
    humidity.clamp(0.0, 100.0) as f32
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;

    use crate::sensors::i2c::fakes::{MockWait, ScriptedI2c, Step};

    use super::*;

    /// The calibration block this module's tests pin (plausible magnitudes;
    /// the temperature vector below reproduces Bosch's API sample result).
    /// Byte layout generated independently (little-endian words, shared
    /// nibble byte for H4/H5) and checked against `Calibration::parse`.
    const TP_CALIB: [u8; 26] = [
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
        0x00, // 0xA0: reserved
        0xC8, // H1 = 200
    ];
    const H_CALIB: [u8; 7] = [
        0x48, 0x71, // H2 = 29000
        0x32, // H3 = 50
        0x17, // H4 msb
        0x48, // H4 lsb nibble (8) | H5 lsb nibble (4)
        0x06, // H5 msb
        0xEC, // H6 = -20
    ];

    /// The data burst for adc_P = 415148, adc_T = 519888, adc_H = 24579.
    const DATA: [u8; 8] = [0x65, 0x5A, 0xC0, 0x7E, 0xED, 0x00, 0x60, 0x03];

    fn calib() -> Calibration {
        Calibration::parse(&TP_CALIB, &H_CALIB)
    }

    /// The init script, as a growable fixture script (`no_std` — heapless,
    /// no `alloc` in this crate).
    fn script() -> heapless::Vec<Step, 32> {
        let mut steps: heapless::Vec<Step, 32> = heapless::Vec::new();
        steps
            .extend_from_slice(&init_script())
            .expect("script depth is a test size");
        steps
    }

    #[test]
    fn calibration_parses_the_shared_nibble_words() {
        let c = calib();
        assert_eq!(c.t1, 27504);
        assert_eq!(c.t2, 26435);
        assert_eq!(c.t3, -1000);
        assert_eq!(c.p1, 36477);
        assert_eq!(c.p9, 6000);
        assert_eq!(c.h1, 200);
        assert_eq!(c.h2, 29000);
        assert_eq!(c.h3, 50);
        assert_eq!(c.h4, 376);
        assert_eq!(c.h5, 100);
        assert_eq!(c.h6, -20);
    }

    #[test]
    fn temperature_matches_the_bosch_sample_vector() {
        // adc_T = 519888 with the Bosch sample trims (T1 = 27504, T2 =
        // 26435, T3 = -1000) → 25.08 °C, the API sample's published result;
        // expected value computed independently from the datasheet formulas.
        let (t, t_fine) = compensate_temperature(519_888, &calib());
        assert!((t - 25.0825).abs() < 1e-3, "t = {t}");
        assert!((t_fine - 128_422.29).abs() < 1e-2, "t_fine = {t_fine}");
    }

    #[test]
    fn temperature_clamps_out_of_range_results() {
        // adc_T = 0 with these trims drives the polynomial below -40.
        let (t, _) = compensate_temperature(0, &calib());
        assert_eq!(t, -40.0);
        // The full 20-bit input (0xFFFFF) drives it above 85.
        let (t, _) = compensate_temperature(0xF_FFFF, &calib());
        assert_eq!(t, 85.0);
    }

    #[test]
    fn pressure_matches_the_reference_vector() {
        let (_, t_fine) = compensate_temperature(519_888, &calib());
        let p = compensate_pressure(415_148, &calib(), t_fine);
        assert!((p - 100_653.2668).abs() < 0.01, "p = {p}");
    }

    #[test]
    fn pressure_degenerate_calibration_reports_the_floor() {
        // With dig_P2 at its ceiling and dig_P3 at its floor, the
        // (1 + var1/32768)·P1 factor goes non-positive for a cold t_fine:
        // the reference reports 30000 rather than dividing by zero.
        let mut c = calib();
        c.p2 = 32_767;
        c.p3 = -32_767;
        let (_, t_fine) = compensate_temperature(0, &c);
        let p = compensate_pressure(415_148, &c, t_fine);
        assert_eq!(p, 30_000.0);
    }

    #[test]
    fn pressure_clamps_out_of_range_results() {
        let (_, t_fine) = compensate_temperature(0, &calib());
        // adc_P = 0 with a deep-negative t_fine extrapolates past 110000.
        let p = compensate_pressure(0, &calib(), t_fine);
        assert_eq!(p, 110_000.0);
    }

    #[test]
    fn humidity_matches_reference_vectors_and_clamps() {
        let (_, t_fine) = compensate_temperature(519_888, &calib());
        let h = compensate_humidity(24_579, &calib(), t_fine);
        assert!((h - 87.284_008).abs() < 1e-4, "h = {h}");
        // Below the physical range → 0, above → 100.
        assert_eq!(compensate_humidity(24_000, &calib(), t_fine), 0.0);
        assert_eq!(compensate_humidity(25_000, &calib(), t_fine), 100.0);
        assert_eq!(compensate_humidity(u16::MAX, &calib(), t_fine), 0.0);
    }

    fn init_script() -> [Step; 5] {
        [
            Step::WriteRead(&[REG_CHIP_ID], &[CHIP_ID]),
            Step::WriteRead(&[REG_TEMP_PRESS_CALIB], &TP_CALIB),
            Step::WriteRead(&[REG_HUM_CALIB], &H_CALIB),
            Step::Write(&[REG_CTRL_HUM, OSRS_X1]),
            Step::Write(&[REG_CTRL_MEAS, CTRL_MEAS_ARMED_SLEEP]),
        ]
    }

    #[test]
    fn init_verifies_identity_reads_trim_and_arms_oversampling() {
        let bus = ScriptedI2c::new(0x76, &init_script());
        let mut driver = Bme280::new(bus, MockWait::new(), 0x76);
        block_on(driver.init()).expect("init succeeds");
        // Bring-up issued exactly the five scripted transactions.
        assert_eq!(driver.bus.transactions(), 5);
        assert!(driver.calib.is_some());
    }

    #[test]
    fn a_wrong_chip_id_is_a_hardware_fault() {
        let steps = [
            Step::WriteRead(&[REG_CHIP_ID], &[0x58]), // BMP280, not BME280
        ];
        let bus = ScriptedI2c::new(0x76, &steps);
        let mut driver = Bme280::new(bus, MockWait::new(), 0x76);
        let error = block_on(driver.init()).expect_err("wrong chip must fail init");
        assert_eq!(error.fault, SensorFault::Hardware);
        assert_eq!(error.code, ERR_CHIP_ID_MISMATCH);
    }

    #[test]
    fn a_nack_on_any_init_transaction_is_a_bus_fault() {
        for (index, step) in init_script().into_iter().enumerate() {
            let mut steps = init_script();
            steps[index] = Step::Fail(BusError::Nack);
            let _ = step; // every position must fail the same way
            let bus = ScriptedI2c::new(0x76, &steps);
            let mut driver = Bme280::new(bus, MockWait::new(), 0x76);
            let error = block_on(driver.init())
                .expect_err("a NACK anywhere in bring-up must fail init");
            assert_eq!(error.fault, SensorFault::Bus);
        }
    }

    #[test]
    fn sample_triggers_forced_conversion_waits_then_bursts_data() {
        let mut steps = script();
        steps
            .push(Step::Write(&[REG_CTRL_MEAS, CTRL_MEAS_FORCED]))
            .expect("script depth is a test size");
        steps
            .push(Step::WriteRead(&[REG_DATA], &DATA))
            .expect("script depth is a test size");

        let bus = ScriptedI2c::new(0x76, &steps);
        let mut driver = Bme280::new(bus, MockWait::new(), 0x76);
        block_on(driver.init()).expect("init succeeds");
        let payload = block_on(driver.sample()).expect("sample succeeds");

        // Conversions waited out: exactly the 20 ms margin.
        assert_eq!(driver.waiter.waits.as_slice(), [CONVERSION_WAIT_MS]);
        // init (5) + trigger + data burst.
        assert_eq!(driver.bus.transactions(), 7);

        let MeasurementPayload::Weather(weather) = payload else {
            panic!("bme280 samples weather");
        };
        assert_eq!(payload.ingest_type(), "weather");
        let (weather_temperature, _) = compensate_temperature(519_888, &calib());
        assert!((weather.temperature.unwrap() - weather_temperature).abs() < 1e-4);
        assert!((weather.pressure.unwrap() - 100_653.2668).abs() < 0.01);
        assert!((weather.humidity.unwrap() - 87.284_008).abs() < 1e-3);
        assert!(weather.precipitation.is_none());
        assert!(weather.rain_probability.is_none());
        assert!(weather.wind_speed.is_none());
    }

    #[test]
    fn invalid_raw_markers_are_reading_faults_not_zero_readings() {
        // All three sentinel patterns map to the same honest "no data".
        // Consts so the fixture can hold `&'static [u8]`.
        const PRESSURE: [u8; 8] = [0x80, 0x00, 0x00, 0x7E, 0xED, 0x00, 0x60, 0x03];
        const TEMPERATURE: [u8; 8] = [0x65, 0x5A, 0xC0, 0x80, 0x00, 0x00, 0x60, 0x03];
        const HUMIDITY: [u8; 8] = [0x65, 0x5A, 0xC0, 0x7E, 0xED, 0x00, 0x80, 0x00];
        for data in [&PRESSURE, &TEMPERATURE, &HUMIDITY] {
            let mut steps = script();
            steps
                .push(Step::Write(&[REG_CTRL_MEAS, CTRL_MEAS_FORCED]))
                .expect("script depth is a test size");
            steps
                .push(Step::WriteRead(&[REG_DATA], data))
                .expect("script depth is a test size");

            let bus = ScriptedI2c::new(0x76, &steps);
            let mut driver = Bme280::new(bus, MockWait::new(), 0x76);
            block_on(driver.init()).expect("init succeeds");
            let error = block_on(driver.sample()).expect_err("sentinel is a fault");
            assert_eq!(error.fault, SensorFault::Reading);
            assert_eq!(error.code, ERR_INVALID_RAW);
        }
    }

    #[test]
    fn a_nack_during_sample_is_a_bus_fault_with_the_operation_code() {
        let mut steps = script();
        steps
            .push(Step::Fail(BusError::Nack)) // the conversion trigger
            .expect("script depth is a test size");
        let bus = ScriptedI2c::new(0x76, &steps);
        let mut driver = Bme280::new(bus, MockWait::new(), 0x76);
        block_on(driver.init()).expect("init succeeds");
        let error = block_on(driver.sample()).expect_err("NACKed trigger");
        assert_eq!(error.fault, SensorFault::Bus);
        assert_eq!(error.code, ERR_TRIGGER_WRITE);
    }

    #[test]
    fn sample_before_init_is_a_misuse_marker_never_a_reading() {
        let bus =
            ScriptedI2c::new(0x76, &[Step::Write(&[REG_CTRL_MEAS, CTRL_MEAS_FORCED])]);
        let mut driver = Bme280::new(bus, MockWait::new(), 0x76);
        let error = block_on(driver.sample()).expect_err("sample before init");
        assert_eq!(error.fault, SensorFault::Reading);
        assert_eq!(error.code, 0);
    }
}
