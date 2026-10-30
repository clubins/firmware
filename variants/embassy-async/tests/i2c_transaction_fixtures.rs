//! End-to-end driver flows over the scripted I²C fixtures.
//!
//! Where the in-module unit tests pin one operation at a time, these run
//! each driver's whole bring-up-and-sample sequence through the public API
//! — init, calibrate, sample — against `ScriptedI2c`, and assert the
//! payloads against the same datasheet vectors the DSP and ingest
//! contracts were built from. Fixtures and their byte scripts are
//! documented with their sources in `sensors::i2c::fakes`.
//!
//! Compiled only when all three I²C drivers are on (`just test-drivers`).

#![cfg(all(
    feature = "sensor-bme280",
    feature = "sensor-sht4x",
    feature = "sensor-scd4x"
))]

use embassy_futures::block_on;

use clubins_firmware_v3::events::MeasurementPayload;
use clubins_firmware_v3::sensor::SensorDriver;
use clubins_firmware_v3::sensors::bme280::Bme280;
use clubins_firmware_v3::sensors::i2c::I2cBus;
use clubins_firmware_v3::sensors::i2c::fakes::{MockWait, ScriptedI2c, Step};
use clubins_firmware_v3::sensors::scd4x::Scd4x;
use clubins_firmware_v3::sensors::sht4x::Sht4x;

// ---- BME280 fixtures (Bosch API sample calibration, datasheet §3.11.2) ------

/// Chip id 0x60 = BME280.
const CHIP_ID: [u8; 1] = [0x60];
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
/// adc_P = 415148, adc_T = 519888, adc_H = 24579.
const DATA: [u8; 8] = [0x65, 0x5A, 0xC0, 0x7E, 0xED, 0x00, 0x60, 0x03];

fn bme280_init() -> Vec<Step> {
    vec![
        // init: identity, both calibration blocks, arm ctrl_hum + ctrl_meas.
        Step::WriteRead(&[0xD0], &CHIP_ID),
        Step::WriteRead(&[0x88], &TP_CALIB),
        Step::WriteRead(&[0xE1], &H_CALIB),
        Step::Write(&[0xF2, 0b001]),
        Step::Write(&[0xF4, 0b001_001_00]),
    ]
}

/// One forced-conversion sample: the trigger write, then the data burst.
fn bme280_cycle() -> Vec<Step> {
    vec![
        Step::Write(&[0xF4, 0b001_001_01]),
        Step::WriteRead(&[0xF7], &DATA),
    ]
}

fn bme280_script() -> Vec<Step> {
    bme280_init().into_iter().chain(bme280_cycle()).collect()
}

// ---- SHT4x fixtures (Sensirion SHT4x datasheet §4.3, §4.5) ------------------

const SHT_SERIAL: [u8; 6] = [0x12, 0x34, 0x37, 0x00, 0x00, 0x81]; // 0x1234_0000
const SHT_DATA: [u8; 6] = [0x66, 0x66, 0x93, 0x80, 0x00, 0xA2];

fn sht4x_script() -> Vec<Step> {
    vec![
        // init: soft reset, serial read (command write, wait, response read).
        Step::Write(&[0x94]),
        Step::Write(&[0x89]),
        Step::Read(&SHT_SERIAL),
        // sample: high-precision measurement.
        Step::Write(&[0xFD]),
        Step::Read(&SHT_DATA),
    ]
}

// ---- SCD4x fixtures (Sensirion SCD4x datasheet §4.4–§4.5) -------------------

const SCD_NOT_READY: [u8; 3] = [0x00, 0x00, 0x81];
const SCD_READY: [u8; 3] = [0x00, 0x01, 0xB0];
const SCD_DATA: [u8; 9] = [0x03, 0x20, 0x2A, 0x66, 0x66, 0x93, 0x80, 0x00, 0xA2];

fn scd4x_script() -> Vec<Step> {
    vec![
        // init: stop a warm loop, arm periodic measurement.
        Step::Write(&[0x3F, 0x86]),
        Step::Write(&[0x21, 0xB1]),
        // sample: data-ready poll misses once, then ready, then measurement.
        Step::Write(&[0xE4, 0xB8]),
        Step::Read(&SCD_NOT_READY),
        Step::Write(&[0xE4, 0xB8]),
        Step::Read(&SCD_READY),
        Step::Write(&[0xEC, 0x05]),
        Step::Read(&SCD_DATA),
    ]
}

// ---- the flows --------------------------------------------------------------

#[test]
fn bme280_bring_up_and_sample_report_the_bosch_vector() {
    let bus = ScriptedI2c::new(0x76, &bme280_script());
    let mut driver = Bme280::new(bus, MockWait::new(), 0x76);
    block_on(driver.init()).expect("bme280 init succeeds");
    block_on(driver.calibrate()).expect("bme280 calibrate succeeds");
    let payload = block_on(driver.sample()).expect("bme280 sample succeeds");
    assert_eq!(payload.ingest_type(), "weather");

    let MeasurementPayload::Weather(weather) = payload else {
        panic!("bme280 samples weather");
    };
    // Bosch sample vector: 25.08 °C, 100653 Pa, 87.28 %RH.
    assert!((weather.temperature.unwrap() - 25.0825).abs() < 1e-3);
    assert!((weather.pressure.unwrap() - 100_653.27).abs() < 0.01);
    assert!((weather.humidity.unwrap() - 87.284).abs() < 1e-2);
    assert!(weather.precipitation.is_none());
    assert!(weather.rain_probability.is_none());
    assert!(weather.wind_speed.is_none());
}

#[test]
fn sht4x_bring_up_and_sample_convert_the_measurement_words() {
    let bus = ScriptedI2c::new(0x44, &sht4x_script());
    let mut driver = Sht4x::new(bus, MockWait::new(), 0x44);
    block_on(driver.init()).expect("sht4x init succeeds");
    block_on(driver.calibrate()).expect("sht4x calibrate succeeds");
    let payload = block_on(driver.sample()).expect("sht4x sample succeeds");

    let MeasurementPayload::Weather(weather) = payload else {
        panic!("sht4x samples weather");
    };
    // Ticks 0x6666/65535 → 25 °C; ticks 0x8000/65535 → 56.5 %RH.
    assert!((weather.temperature.unwrap() - 25.0).abs() < 1e-4);
    assert!((weather.humidity.unwrap() - 56.501).abs() < 1e-2);
    assert!(weather.pressure.is_none());
}

#[test]
fn scd4x_bring_up_and_sample_report_the_co2_reading() {
    let bus = ScriptedI2c::new(0x62, &scd4x_script());
    let mut driver = Scd4x::new(bus, MockWait::new(), 0x62);
    block_on(driver.init()).expect("scd4x init succeeds");
    block_on(driver.calibrate()).expect("scd4x calibrate succeeds");
    let payload = block_on(driver.sample()).expect("scd4x sample succeeds");
    assert_eq!(payload.ingest_type(), "co2");

    let MeasurementPayload::Co2(co2) = payload else {
        panic!("scd4x samples co2");
    };
    assert_eq!(co2.ppm, 800.0);
}

#[test]
fn a_sensor_that_never_becomes_ready_is_a_reading_fault() {
    // The SCD4x poll budget (6 × 1 s) exhausted end to end: the framework
    // receives a Reading fault, not a stalled task.
    let mut steps = Vec::new();
    steps.extend_from_slice(&[Step::Write(&[0x3F, 0x86]), Step::Write(&[0x21, 0xB1])]);
    for _ in 0..7 {
        steps.push(Step::Write(&[0xE4, 0xB8]));
        steps.push(Step::Read(&SCD_NOT_READY));
    }

    let bus = ScriptedI2c::new(0x62, &steps);
    let mut driver = Scd4x::new(bus, MockWait::new(), 0x62);
    block_on(driver.init()).expect("scd4x init succeeds");
    let error = block_on(driver.sample()).expect_err("never-ready is a fault");
    assert_eq!(
        error.fault,
        clubins_firmware_v3::sensor::SensorFault::Reading
    );
}

#[test]
fn the_fixture_repeats_its_final_step_when_the_script_runs_out() {
    // A flow that repeats one transaction kind — polling a register, say —
    // keeps getting the final scripted response after the script ends.
    // (Driver sample cycles alternate kinds, so they script every cycle:
    // see repeated_sample_cycles_stay_stable_end_to_end.)
    block_on(async {
        let mut bus = ScriptedI2c::new(0x76, &[Step::Read(&[0x42])]);
        for _ in 0..3 {
            let mut register = [0u8; 1];
            bus.read(0x76, &mut register).await.expect("read succeeds");
            assert_eq!(register, [0x42]);
        }
        assert_eq!(bus.transactions(), 3);
    });
}

#[test]
fn repeated_sample_cycles_stay_stable_end_to_end() {
    // Both cycles scripted exactly (init 5 transactions + trigger/burst
    // per sample): every sample runs a fresh forced conversion, and
    // consecutive readings match.
    let mut steps: Vec<Step> = Vec::new();
    steps.extend(bme280_init());
    steps.extend(bme280_cycle());
    steps.extend(bme280_cycle());

    let bus = ScriptedI2c::new(0x76, &steps);
    let mut driver = Bme280::new(bus, MockWait::new(), 0x76);
    block_on(driver.init()).expect("bme280 init succeeds");
    let first = block_on(driver.sample()).expect("first sample succeeds");
    let second = block_on(driver.sample()).expect("second sample succeeds");

    assert_eq!(first, second);
}
