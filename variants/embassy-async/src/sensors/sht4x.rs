//! SHT4x — Sensirion temperature / humidity over I²C
//! (`sensor-sht4x` → `weather`).
//!
//! Commands, timing, and conversions per the Sensirion SHT4x datasheet
//! (v7.3 §4.5 command table, §4.13 conversions, §4.9 CRC), retrieved this
//! session: high-precision measurement `0xFD`, soft reset `0x94`, serial
//! number `0x89`; `T = −45 + 175·St/65535`,
//! `RH = −6 + 125·SRH/65535` clamped to 0–100; every 16-bit word is
//! big-endian and followed by its CRC-8 (poly 0x31, init 0xFF).
//!
//! The sensor is command-driven and stateless: each `sample` sends `0xFD`,
//! waits out the measurement window, then reads the 6-byte response. Bring-up
//! resets the sensor and reads its serial number — the identity the status
//! event reports — but serials are not per-unit calibration: the SHT4x ships
//! factory-calibrated.
//!
//! Error codes (`SensorError.code`, this driver's causes):
//!
//! | code | fault | meaning |
//! |------|-------|---------|
//! | 1 | Bus | soft-reset write failed |
//! | 2 | Bus | serial read failed |
//! | 3 | Reading | serial CRC mismatch — corrupted identity, never reported |
//! | 4 | Bus | measurement trigger failed |
//! | 5 | Bus | measurement read failed |
//! | 6 | Reading | measurement word CRC mismatch |

use crate::events::{MeasurementPayload, WeatherSample};
use crate::sensor::{SensorDriver, SensorError, SensorFault};
use crate::sensors::Wait;
use crate::sensors::i2c::{BusError, I2cBus};
use crate::sensors::sensirion::checked_word;

// Commands (SHT4x datasheet §4.5).
const CMD_SOFT_RESET: u8 = 0x94;
const CMD_READ_SERIAL: u8 = 0x89;
const CMD_MEASURE_HIGH_PRECISION: u8 = 0xFD;

/// Soft-reset completion window (datasheet: up to 1 ms; margin included).
const RESET_WAIT_MS: u64 = 2;
/// Serial read command-execution time (datasheet command table).
const SERIAL_WAIT_MS: u64 = 2;
/// High-precision measurement window: covers the datasheet's worst-case
/// conversion (retrieved vendor tables put typical at 3.7 ms, max under
/// 5 ms; 10 ms is the settled margin).
const MEASURE_WAIT_MS: u64 = 10;

// Error codes — see the module table.
const ERR_RESET_WRITE: u8 = 1;
const ERR_SERIAL_READ: u8 = 2;
const ERR_SERIAL_CRC: u8 = 3;
const ERR_TRIGGER_WRITE: u8 = 4;
const ERR_MEASURE_READ: u8 = 5;
const ERR_CRC: u8 = 6;

/// The SHT4x driver: bus handle + wait seam + the serial identity.
pub struct Sht4x<B, W> {
    bus: B,
    waiter: W,
    address: u8,
    serial: u32,
}

impl<B, W> Sht4x<B, W> {
    /// Build a driver for the sensor at `address` (0x44 with ADDR low)
    /// over `bus`, waiting out command execution with `waiter`.
    #[must_use]
    pub const fn new(bus: B, waiter: W, address: u8) -> Self {
        Self {
            bus,
            waiter,
            address,
            serial: 0,
        }
    }

    fn bus_fault(error: BusError, code: u8) -> SensorError {
        let _ = error;
        SensorError::new(SensorFault::Bus, code)
    }

    /// The 32-bit serial number read at bring-up; `0` before `init`.
    #[must_use]
    pub const fn serial(&self) -> u32 {
        self.serial
    }
}

impl<B: I2cBus, W: Wait> SensorDriver for Sht4x<B, W> {
    async fn init(&mut self) -> Result<(), SensorError> {
        // Reset to a known state, then read the identity. A serial whose
        // CRC does not check is a corrupted bus, not a serial to report.
        self.bus
            .write(self.address, &[CMD_SOFT_RESET])
            .await
            .map_err(|error| Self::bus_fault(error, ERR_RESET_WRITE))?;
        self.waiter.wait(RESET_WAIT_MS).await;

        let mut response = [0u8; 6];
        self.bus
            .write(self.address, &[CMD_READ_SERIAL])
            .await
            .map_err(|error| Self::bus_fault(error, ERR_SERIAL_READ))?;
        self.waiter.wait(SERIAL_WAIT_MS).await;
        self.bus
            .read(self.address, &mut response)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_SERIAL_READ))?;
        let word1 = checked_word(&response, 0)
            .ok_or_else(|| SensorError::new(SensorFault::Reading, ERR_SERIAL_CRC))?;
        let word2 = checked_word(&response, 3)
            .ok_or_else(|| SensorError::new(SensorFault::Reading, ERR_SERIAL_CRC))?;
        self.serial = u32::from_be_bytes([
            (word1 >> 8) as u8,
            word1 as u8,
            (word2 >> 8) as u8,
            word2 as u8,
        ]);
        Ok(())
    }

    async fn calibrate(&mut self) -> Result<(), SensorError> {
        // Factory-calibrated; the framework's per-unit hook is a no-op.
        Ok(())
    }

    async fn sample(&mut self) -> Result<MeasurementPayload, SensorError> {
        self.bus
            .write(self.address, &[CMD_MEASURE_HIGH_PRECISION])
            .await
            .map_err(|error| Self::bus_fault(error, ERR_TRIGGER_WRITE))?;
        self.waiter.wait(MEASURE_WAIT_MS).await;

        let mut response = [0u8; 6];
        self.bus
            .read(self.address, &mut response)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_MEASURE_READ))?;
        let ticks_t = checked_word(&response, 0)
            .ok_or_else(|| SensorError::new(SensorFault::Reading, ERR_CRC))?;
        let ticks_rh = checked_word(&response, 3)
            .ok_or_else(|| SensorError::new(SensorFault::Reading, ERR_CRC))?;

        Ok(MeasurementPayload::Weather(WeatherSample {
            temperature: Some(convert_temperature(ticks_t)),
            humidity: Some(convert_humidity(ticks_rh)),
            // Pressure is a BME280 channel; the virtual Open-Meteo fields
            // stay server-side (see the BME280 driver).
            pressure: None,
            precipitation: None,
            rain_probability: None,
            wind_speed: None,
        }))
    }
}

/// Temperature from the raw ticks: −45 + 175·St/65535 (datasheet §4.13).
/// The formula's full-scale output (130 °C) is the conversion itself —
/// the datasheet defines no temperature clamp, unlike humidity.
fn convert_temperature(ticks: u16) -> f32 {
    -45.0 + 175.0 * f32::from(ticks) / 65_535.0
}

/// Relative humidity from the raw ticks: −6 + 125·SRH/65535, limited to
/// 0–100 % (datasheet: "resulting relative humidity should generally be
/// limited to 0–100 %").
fn convert_humidity(ticks: u16) -> f32 {
    (-6.0 + 125.0 * f32::from(ticks) / 65_535.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;

    use crate::sensors::i2c::fakes::{MockWait, ScriptedI2c, Step};
    use crate::sensors::sensirion::crc8;

    use super::*;

    /// Serial response: words 0x1234 and 0x0000 with their CRCs (0x37 and
    /// 0x81, generated independently with the same parameters).
    const SERIAL_RESPONSE: [u8; 6] = [0x12, 0x34, 0x37, 0x00, 0x00, 0x81];
    /// Measurement response: temperature ticks 0x6666 (CRC 0x93), humidity
    /// ticks 0x8000 (CRC 0xA2).
    const MEASUREMENT: [u8; 6] = [0x66, 0x66, 0x93, 0x80, 0x00, 0xA2];

    fn init_script() -> [Step; 3] {
        [
            Step::Write(&[CMD_SOFT_RESET]),
            Step::Write(&[CMD_READ_SERIAL]),
            Step::Read(&SERIAL_RESPONSE),
        ]
    }

    #[test]
    fn crc8_sensirion_word_check_is_pinned() {
        // The fixture bytes themselves: both words of the serial response
        // and both words of the measurement must CRC-check.
        assert_eq!(crc8(&[0x12, 0x34]), 0x37);
        assert_eq!(crc8(&[0x00, 0x00]), 0x81);
        assert_eq!(crc8(&[0x66, 0x66]), 0x93);
        assert_eq!(crc8(&[0x80, 0x00]), 0xA2);
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
    fn conversions_match_the_datasheet_vectors() {
        // Ticks 26214 = 0.4 full scale → exactly 25 °C; ticks 32768 = 0.5
        // → −6 + 62.5 = 56.5 %.
        let t = convert_temperature(26_214);
        assert!((t - 25.0).abs() < 1e-4, "t = {t}");
        let rh = convert_humidity(32_768);
        assert!((rh - 56.500_954).abs() < 1e-4, "rh = {rh}");
        // Full scale: the formula's own output (130 °C) — no temperature
        // clamp; humidity pins at 100.
        let t = convert_temperature(u16::MAX);
        assert!((t - 130.0).abs() < 1e-4, "t = {t}");
        assert_eq!(convert_humidity(u16::MAX), 100.0);
        assert_eq!(convert_humidity(0), 0.0);
    }

    #[test]
    fn init_resets_and_reads_the_serial() {
        let bus = ScriptedI2c::new(0x44, &init_script());
        let mut waiter = MockWait::new();
        {
            let mut driver = Sht4x::new(bus, &mut waiter, 0x44);
            block_on(driver.init()).expect("init succeeds");
            assert_eq!(driver.serial(), 0x1234_0000);
        }
        // Reset (≤1 ms) and the serial command window were both waited.
        assert_eq!(waiter.waits.as_slice(), [RESET_WAIT_MS, SERIAL_WAIT_MS]);
    }

    #[test]
    fn corrupt_serial_is_a_reading_fault_not_a_zero_identity() {
        let steps = [
            Step::Write(&[CMD_SOFT_RESET]),
            Step::Write(&[CMD_READ_SERIAL]),
            Step::Read(&[0x12, 0x34, 0x38, 0x00, 0x00, 0x81]), // bad CRC
        ];
        let bus = ScriptedI2c::new(0x44, &steps);
        let mut driver = Sht4x::new(bus, MockWait::new(), 0x44);
        let error = block_on(driver.init()).expect_err("corrupt serial must fail init");
        assert_eq!(error.fault, SensorFault::Reading);
        assert_eq!(error.code, ERR_SERIAL_CRC);
    }

    #[test]
    fn sample_converts_the_measurement_words() {
        let mut steps = script();
        steps
            .push(Step::Write(&[CMD_MEASURE_HIGH_PRECISION]))
            .expect("script depth is a test size");
        steps
            .push(Step::Read(&MEASUREMENT))
            .expect("script depth is a test size");

        let bus = ScriptedI2c::new(0x44, &steps);
        let mut waiter = MockWait::new();
        let mut driver = Sht4x::new(bus, &mut waiter, 0x44);
        block_on(driver.init()).expect("init succeeds");
        let payload = block_on(driver.sample()).expect("sample succeeds");
        assert_eq!(payload.ingest_type(), "weather");

        let MeasurementPayload::Weather(weather) = payload else {
            panic!("sht4x samples weather");
        };
        assert!((weather.temperature.unwrap() - 25.0).abs() < 1e-4);
        assert!((weather.humidity.unwrap() - 56.500_954).abs() < 1e-3);
        assert!(weather.pressure.is_none());
        assert!(weather.precipitation.is_none());
        assert!(weather.rain_probability.is_none());
        assert!(weather.wind_speed.is_none());

        // Flow: reset → serial → read serial → trigger → read — with the
        // command-execution waits in between.
        assert_eq!(
            driver.waiter.waits.as_slice(),
            [RESET_WAIT_MS, SERIAL_WAIT_MS, MEASURE_WAIT_MS]
        );
        assert_eq!(driver.bus.transactions(), 5);
    }

    #[test]
    fn corrupt_measurement_words_are_reading_faults() {
        // Bad temperature CRC and bad humidity CRC, separately. Const
        // payloads so the fixture can hold `&'static [u8]`.
        const BAD_T: [u8; 6] = [0x66, 0x66, 0x94, 0x80, 0x00, 0xA2];
        const BAD_RH: [u8; 6] = [0x66, 0x66, 0x93, 0x80, 0x00, 0xA3];
        for (data, code) in [(&BAD_T, ERR_CRC), (&BAD_RH, ERR_CRC)] {
            let mut steps = script();
            steps
                .push(Step::Write(&[CMD_MEASURE_HIGH_PRECISION]))
                .expect("script depth is a test size");
            steps
                .push(Step::Read(data))
                .expect("script depth is a test size");
            let bus = ScriptedI2c::new(0x44, &steps);
            let mut driver = Sht4x::new(bus, MockWait::new(), 0x44);
            block_on(driver.init()).expect("init succeeds");
            let error = block_on(driver.sample()).expect_err("corrupt word");
            assert_eq!(error.fault, SensorFault::Reading);
            assert_eq!(error.code, code);
        }
    }

    #[test]
    fn a_nack_on_the_trigger_is_a_bus_fault() {
        let mut steps = script();
        steps
            .push(Step::Fail(crate::sensors::i2c::BusError::Nack))
            .expect("script depth is a test size");
        let bus = ScriptedI2c::new(0x44, &steps);
        let mut driver = Sht4x::new(bus, MockWait::new(), 0x44);
        block_on(driver.init()).expect("init succeeds");
        let error = block_on(driver.sample()).expect_err("NACKed trigger");
        assert_eq!(error.fault, SensorFault::Bus);
        assert_eq!(error.code, ERR_TRIGGER_WRITE);
    }
}
