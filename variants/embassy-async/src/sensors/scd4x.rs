//! SCD4x — Sensirion true-CO₂ / temperature / humidity over I²C
//! (`sensor-scd4x` → `co2`).
//!
//! Commands, timing, and conversions per the Sensirion SCD4x datasheet
//! (D1, §3.5 command sequences, §3.11 CRC), retrieved this session:
//! start periodic measurement `0x21B1` (5 s signal update interval),
//! data-ready status `0xE4B8`, read measurement `0xEC05`, stop periodic
//! measurement `0x3F86`; conversions `CO₂ = raw` ppm,
//! `T = −45 + 175·t_raw/65536`, `RH = 100·h_raw/65536`; every 16-bit word
//! big-endian with its CRC-8 (poly 0x31, init 0xFF); data-ready tests the
//! **low 11 bits** of the status word (any set → data available).
//!
//! The sensor runs Sensirion's periodic mode: init stops any conversion a
//! warm reboot left running, then arms the 5 s measurement loop; each
//! `sample` polls the data-ready word (the loop ticks asynchronously of the
//! task's own cadence) and reads the 9-byte measurement burst.
//!
//! No output sentinel is checked beyond the data-ready gate: in periodic
//! mode a ready flag precedes every read, and each word's CRC makes
//! corruption a fault rather than a value.
//!
//! Error codes (`SensorError.code`, this driver's causes):
//!
//! | code | fault | meaning |
//! |------|-------|---------|
//! | 1 | Bus | stop-periodic write failed |
//! | 2 | Bus | start-periodic write failed |
//! | 3 | Bus | data-ready read failed |
//! | 4 | Reading | measurement still not ready after all polls |
//! | 5 | Bus | measurement read failed |
//! | 6 | Reading | measurement word CRC mismatch |

use crate::events::{Co2Sample, MeasurementPayload};
use crate::sensor::{SensorDriver, SensorError, SensorFault};
use crate::sensors::Wait;
use crate::sensors::i2c::{BusError, I2cBus};
use crate::sensors::sensirion::checked_word;

// Commands (SCD4x datasheet §3.5), high byte first.
const CMD_STOP_PERIODIC: [u8; 2] = [0x3F, 0x86];
const CMD_START_PERIODIC: [u8; 2] = [0x21, 0xB1];
const CMD_DATA_READY: [u8; 2] = [0xE4, 0xB8];
const CMD_READ_MEASUREMENT: [u8; 2] = [0xEC, 0x05];

/// Command-execution time of the read commands (datasheet: ~1 ms between
/// sending the command and reading the response).
const COMMAND_WAIT_MS: u64 = 1;
/// Stop-periodic execution window before re-arming (datasheet command
/// table; the largest timed command in this driver's flow).
const STOP_WAIT_MS: u64 = 500;
/// How long to wait between "not ready" polls. The measurement loop ticks
/// every 5 s, so one interval is the natural spacing.
const READY_POLL_MS: u64 = 1_000;
/// Polls before declaring the measurement stuck: a full measurement period
/// (5 s) plus one interval of grace.
const READY_MAX_POLLS: u32 = 6;

// Data-ready mask: the datasheet defines readiness on the status word's
// low 11 bits (0x7FF), not "nonzero".
const DATA_READY_MASK: u16 = 0x07FF;

// Error codes — see the module table.
const ERR_STOP_WRITE: u8 = 1;
const ERR_START_WRITE: u8 = 2;
const ERR_READY_READ: u8 = 3;
const ERR_NOT_READY: u8 = 4;
const ERR_MEASURE_READ: u8 = 5;
const ERR_CRC: u8 = 6;

/// The SCD4x driver: bus handle + wait seam.
pub struct Scd4x<B, W> {
    bus: B,
    waiter: W,
    address: u8,
}

impl<B, W> Scd4x<B, W> {
    /// Build a driver for the sensor at `address` (0x62, fixed) over
    /// `bus`, waiting out command execution with `waiter`.
    #[must_use]
    pub const fn new(bus: B, waiter: W, address: u8) -> Self {
        Self {
            bus,
            waiter,
            address,
        }
    }

    fn bus_fault(error: BusError, code: u8) -> SensorError {
        let _ = error;
        SensorError::new(SensorFault::Bus, code)
    }
}

impl<B: I2cBus, W: Wait> Scd4x<B, W> {
    /// One data-ready poll: command, ~1 ms execution window, 3-byte
    /// CRC-checked status. Readiness is the low 11 bits (datasheet §4.4).
    async fn data_ready(&mut self) -> Result<bool, SensorError> {
        let mut response = [0u8; 3];
        self.bus
            .write(self.address, &CMD_DATA_READY)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_READY_READ))?;
        self.waiter.wait(COMMAND_WAIT_MS).await;
        self.bus
            .read(self.address, &mut response)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_READY_READ))?;
        let status = checked_word(&response, 0)
            .ok_or_else(|| SensorError::new(SensorFault::Reading, ERR_CRC))?;
        Ok(status & DATA_READY_MASK != 0)
    }
}

impl<B: I2cBus, W: Wait> SensorDriver for Scd4x<B, W> {
    async fn init(&mut self) -> Result<(), SensorError> {
        // A warm reboot does not power-cycle the sensor: stop whatever
        // periodic loop a previous run left armed, then start fresh. Stop
        // on an already-idle sensor is harmless.
        self.bus
            .write(self.address, &CMD_STOP_PERIODIC)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_STOP_WRITE))?;
        self.waiter.wait(STOP_WAIT_MS).await;
        self.bus
            .write(self.address, &CMD_START_PERIODIC)
            .await
            .map_err(|error| Self::bus_fault(error, ERR_START_WRITE))?;
        Ok(())
    }

    async fn calibrate(&mut self) -> Result<(), SensorError> {
        // Factory-calibrated NDIR; the framework's per-unit hook is a no-op.
        // (FASC/ASC offset corrections are a provisioning-time concern.)
        Ok(())
    }

    async fn sample(&mut self) -> Result<MeasurementPayload, SensorError> {
        // The sensor's own loop ticks every 5 s, independently of the task
        // cadence; poll the ready word until data shows up or the budget
        // (one full period plus grace) runs out.
        for _ in 0..READY_MAX_POLLS {
            if self.data_ready().await? {
                let mut response = [0u8; 9];
                self.bus
                    .write(self.address, &CMD_READ_MEASUREMENT)
                    .await
                    .map_err(|error| Self::bus_fault(error, ERR_MEASURE_READ))?;
                self.waiter.wait(COMMAND_WAIT_MS).await;
                self.bus
                    .read(self.address, &mut response)
                    .await
                    .map_err(|error| Self::bus_fault(error, ERR_MEASURE_READ))?;

                let co2_raw = checked_word(&response, 0)
                    .ok_or_else(|| SensorError::new(SensorFault::Reading, ERR_CRC))?;
                // The temperature and humidity words are part of the same
                // CRC-checked frame — parsed for integrity, dropped as
                // values: the SCD4x's auxiliaries are not the weather
                // ingest, and CO₂ is this sensor's only payload.
                let _ticks_t = checked_word(&response, 3)
                    .ok_or_else(|| SensorError::new(SensorFault::Reading, ERR_CRC))?;
                let _ticks_rh = checked_word(&response, 6)
                    .ok_or_else(|| SensorError::new(SensorFault::Reading, ERR_CRC))?;

                return Ok(MeasurementPayload::Co2(Co2Sample {
                    ppm: convert_co2(co2_raw),
                }));
            }
            self.waiter.wait(READY_POLL_MS).await;
        }
        Err(SensorError::new(SensorFault::Reading, ERR_NOT_READY))
    }
}

/// CO₂ concentration from the raw word: the raw value IS ppm (0..40000).
fn convert_co2(raw: u16) -> f32 {
    f32::from(raw)
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;

    use crate::sensors::i2c::fakes::{MockWait, ScriptedI2c, Step};

    use super::*;

    /// Data-ready: not ready (0x0000) then ready (0x0001, low 11 bits set).
    const NOT_READY: [u8; 3] = [0x00, 0x00, 0x81];
    const READY: [u8; 3] = [0x00, 0x01, 0xB0];
    /// Measurement: co2 800 (0x0320), temperature ticks 0x6666, humidity
    /// ticks 0x8000 — each word CRC-checked (0x2A, 0x93, 0xA2).
    const MEASUREMENT: [u8; 9] = [0x03, 0x20, 0x2A, 0x66, 0x66, 0x93, 0x80, 0x00, 0xA2];

    fn init_script() -> [Step; 2] {
        [
            Step::Write(&CMD_STOP_PERIODIC),
            Step::Write(&CMD_START_PERIODIC),
        ]
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
        // 800 ppm is the ambient reference. The aux-word conversions
        // (−45 + 175·t/65536, 100·h/65536) are deliberately not
        // implemented — the SCD4x payload is CO₂-only — but the frame
        // CRCs pin the words regardless.
        assert_eq!(convert_co2(800), 800.0);
        assert_eq!(convert_co2(0), 0.0);
        assert_eq!(convert_co2(40_000), 40_000.0);
    }

    #[test]
    fn data_ready_tests_the_low_eleven_bits() {
        // Per the datasheet, readiness is (status & 0x7FF) != 0 — the
        // fixture words 0x0000 (not ready) and 0x0001 (ready) straddle it.
        let status_not = u16::from_be_bytes([NOT_READY[0], NOT_READY[1]]);
        assert_eq!(status_not & DATA_READY_MASK, 0);
        let status_yes = u16::from_be_bytes([READY[0], READY[1]]);
        assert_ne!(status_yes & DATA_READY_MASK, 0);
        // A word with only high bits set (0x0800) is NOT ready.
        assert_eq!(0x0800 & DATA_READY_MASK, 0);
    }

    #[test]
    fn init_stops_a_warm_loop_then_arms_periodic_mode() {
        let bus = ScriptedI2c::new(0x62, &init_script());
        let mut waiter = MockWait::new();
        {
            let mut driver = Scd4x::new(bus, &mut waiter, 0x62);
            block_on(driver.init()).expect("init succeeds");
        }
        assert_eq!(waiter.waits.as_slice(), [STOP_WAIT_MS]);
    }

    #[test]
    fn sample_polls_ready_then_reads_the_measurement() {
        let mut steps = script();
        // First poll: not ready → 1 s spacing → second poll: ready →
        // measurement read.
        steps
            .push(Step::Write(&CMD_DATA_READY))
            .expect("script fits");
        steps.push(Step::Read(&NOT_READY)).expect("script fits");
        steps
            .push(Step::Write(&CMD_DATA_READY))
            .expect("script fits");
        steps.push(Step::Read(&READY)).expect("script fits");
        steps
            .push(Step::Write(&CMD_READ_MEASUREMENT))
            .expect("script fits");
        steps.push(Step::Read(&MEASUREMENT)).expect("script fits");

        let bus = ScriptedI2c::new(0x62, &steps);
        let mut waiter = MockWait::new();
        let mut driver = Scd4x::new(bus, &mut waiter, 0x62);
        block_on(driver.init()).expect("init succeeds");
        let payload = block_on(driver.sample()).expect("sample succeeds");
        assert_eq!(payload.ingest_type(), "co2");

        let MeasurementPayload::Co2(co2) = payload else {
            panic!("scd4x samples co2");
        };
        assert_eq!(co2.ppm, 800.0);

        // The ready poll waited one 1 s spacing; every command got its
        // ~1 ms execution window.
        assert_eq!(
            driver.waiter.waits.as_slice(),
            [
                STOP_WAIT_MS,
                COMMAND_WAIT_MS,
                READY_POLL_MS,
                COMMAND_WAIT_MS,
                COMMAND_WAIT_MS
            ]
        );
        assert_eq!(driver.bus.transactions(), 8);
    }

    #[test]
    fn sample_gives_up_after_the_poll_budget() {
        let mut steps = script();
        // Six not-ready polls exhaust the budget.
        for _ in 0..READY_MAX_POLLS {
            steps
                .push(Step::Write(&CMD_DATA_READY))
                .expect("script fits");
            steps.push(Step::Read(&NOT_READY)).expect("script fits");
        }

        let bus = ScriptedI2c::new(0x62, &steps);
        let mut waiter = MockWait::new();
        let mut driver = Scd4x::new(bus, &mut waiter, 0x62);
        block_on(driver.init()).expect("init succeeds");
        let error = block_on(driver.sample()).expect_err("a stuck sensor");
        assert_eq!(error.fault, SensorFault::Reading);
        assert_eq!(error.code, ERR_NOT_READY);
        // Each of the six polls cost a 1 ms command window plus the 1 s
        // spacing, after the init stop window.
        let mut expected: heapless::Vec<u64, 16> = heapless::Vec::new();
        expected
            .push(STOP_WAIT_MS)
            .expect("wait log is a test size");
        for _ in 0..READY_MAX_POLLS {
            expected
                .push(COMMAND_WAIT_MS)
                .expect("wait log is a test size");
            expected
                .push(READY_POLL_MS)
                .expect("wait log is a test size");
        }
        assert_eq!(driver.waiter.waits.as_slice(), expected.as_slice());
    }

    #[test]
    fn a_crc_failure_on_the_status_word_is_a_reading_fault() {
        let mut steps = script();
        steps
            .push(Step::Write(&CMD_DATA_READY))
            .expect("script fits");
        steps
            .push(Step::Read(&[0x00, 0x01, 0xB1])) // corrupt ready CRC
            .expect("script fits");
        let bus = ScriptedI2c::new(0x62, &steps);
        let mut driver = Scd4x::new(bus, MockWait::new(), 0x62);
        block_on(driver.init()).expect("init succeeds");
        let error = block_on(driver.sample()).expect_err("corrupt status word");
        assert_eq!(error.fault, SensorFault::Reading);
        assert_eq!(error.code, ERR_CRC);
    }

    #[test]
    fn a_nack_on_the_measurement_read_is_a_bus_fault() {
        let mut steps = script();
        steps
            .push(Step::Write(&CMD_DATA_READY))
            .expect("script fits");
        steps.push(Step::Read(&READY)).expect("script fits");
        steps
            .push(Step::Write(&CMD_READ_MEASUREMENT))
            .expect("script fits");
        steps.push(Step::Fail(BusError::Nack)).expect("script fits");
        let bus = ScriptedI2c::new(0x62, &steps);
        let mut driver = Scd4x::new(bus, MockWait::new(), 0x62);
        block_on(driver.init()).expect("init succeeds");
        let error = block_on(driver.sample()).expect_err("NACKed measurement");
        assert_eq!(error.fault, SensorFault::Bus);
        assert_eq!(error.code, ERR_MEASURE_READ);
    }

    #[test]
    fn a_nack_during_init_is_a_bus_fault() {
        let steps = [
            Step::Fail(BusError::Timeout), // stop-periodic fails
            Step::Write(&CMD_START_PERIODIC),
        ];
        let bus = ScriptedI2c::new(0x62, &steps);
        let mut driver = Scd4x::new(bus, MockWait::new(), 0x62);
        let error = block_on(driver.init()).expect_err("stop failed");
        assert_eq!(error.fault, SensorFault::Bus);
        assert_eq!(error.code, ERR_STOP_WRITE);
    }
}
