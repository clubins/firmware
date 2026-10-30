//! End-to-end PMS5003 frame fixtures: the whole bring-up-and-sample flow
//! over the scripted UART.
//!
//! Where the in-module unit tests pin the parser one byte at a time,
//! these run the driver's public contract — init (passive-mode arming),
//! calibrate, sample — against `ScriptedUart` with datasheet-real frames.
//! They pin the behaviour the ingest topic depends on: atmospheric PM2.5
//! extraction, reassembly across read boundaries, acknowledgement frames
//! that resync instead of faulting, and which failure shapes fault
//! instead of fabricating a reading. The frame layout follows the
//! Plantower PMS5003 datasheet, cross-checked against the V5 variant's
//! driver (see `sensors::pms5003`). Command bytes on the wire are pinned
//! by the in-module unit tests, which can reach the private port handle.
//!
//! Compiled with the PMS5003 driver (`just test-drivers`).

#![cfg(feature = "sensor-pms5003")]

use embassy_futures::block_on;

use clubins_firmware_v3::events::{MeasurementPayload, Pm25Sample};
use clubins_firmware_v3::sensor::{SensorDriver, SensorFault};
use clubins_firmware_v3::sensors::pms5003::Pms5003;
use clubins_firmware_v3::sensors::uart::UartError;
use clubins_firmware_v3::sensors::uart::fakes::{MockWait, ReadStep, ScriptedUart};

// ---- frame fixtures (Plantower PMS5003 datasheet) --------------------------

/// Build a 32-byte data frame at compile time: start pair, frame-length
/// word (2×13 + 2), thirteen big-endian words, and the 16-bit sum of the
/// first 30 bytes as the checksum. The thirteenth word packs the firmware
/// version (high byte) and the sensor's error bits (low byte).
const fn frame(words: [u16; 13]) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[0] = 0x42;
    bytes[1] = 0x4D;
    bytes[2] = 0x00;
    bytes[3] = 28;
    let mut i = 0;
    while i < 13 {
        let word = words[i];
        bytes[4 + 2 * i] = (word >> 8) as u8;
        bytes[5 + 2 * i] = (word & 0xff) as u8;
        i += 1;
    }
    let mut checksum: u16 = 0;
    let mut j = 0;
    while j < 30 {
        checksum += bytes[j] as u16;
        j += 1;
    }
    bytes[30] = (checksum >> 8) as u8;
    bytes[31] = (checksum & 0xff) as u8;
    bytes
}

/// One clean atmospheric reading: PM2.5 = 10 µg/m³, firmware version 100,
/// no sensor error bits.
static CLEAN_FRAME: [u8; 32] = frame([
    7,
    12,
    20, // CF=1 (industrial): PM1.0, PM2.5, PM10.0
    6,
    10,
    18, // atmospheric: PM1.0, PM2.5 = 10 µg/m³, PM10.0
    0x0100,
    0x0200,
    0x0300,
    0x0400,
    0x0500,
    0x0600,            // per-0.1 L counts
    (100 << 8) | 0x00, // version 100, error bits clear
]);

/// The clean frame with one concentration byte flipped: the frame shape
/// is valid, the checksum is not.
static CORRUPTED_FRAME: [u8; 32] = {
    let mut bytes = CLEAN_FRAME;
    bytes[9] ^= 0x01;
    bytes
};

/// Version 100, error bits 0x05: the sensor reports its own fault.
static FAULTY_FRAME: [u8; 32] = frame([
    7,
    12,
    20,
    6,
    10,
    18,
    0x0100,
    0x0200,
    0x0300,
    0x0400,
    0x0500,
    0x0600,
    (100 << 8) | 0x05, // version 100, sensor error bits 0x05
]);

/// A command acknowledgement as some firmware revisions emit it: the
/// command echo with a data word, never a 32-byte data frame.
static COMMAND_ACK: [u8; 7] = [0x42, 0x4D, 0xE2, 0x00, 0x00, 0x01, 0x72];

#[test]
fn sample_publishes_atmospheric_pm25() {
    block_on(async {
        let mut pms = Pms5003::new(
            ScriptedUart::new(&[ReadStep::Bytes(&CLEAN_FRAME)]),
            MockWait::new(),
        );
        pms.init().await.expect("init succeeds");
        pms.calibrate().await.expect("calibrate is a no-op");
        let payload = pms.sample().await.expect("a clean frame yields a reading");
        assert_eq!(
            payload,
            MeasurementPayload::Pm25(Pm25Sample { ug_m3: 10.0 })
        );
    });
}

#[test]
fn frame_reassembles_across_read_boundaries_and_garbage() {
    block_on(async {
        // Garbage prefix, then the start pair split across two reads, then
        // the rest of the frame: reassembly and start-pair resync must
        // both survive arbitrary chunking.
        let mut pms = Pms5003::new(
            ScriptedUart::new(&[
                ReadStep::Bytes(&[0x00, 0xFF, 0x42]),
                ReadStep::Bytes(&CLEAN_FRAME[1..15]),
                ReadStep::Bytes(&CLEAN_FRAME[15..]),
            ]),
            MockWait::new(),
        );
        pms.init().await.expect("init succeeds");
        let payload = pms
            .sample()
            .await
            .expect("the frame still yields a reading");
        assert_eq!(
            payload,
            MeasurementPayload::Pm25(Pm25Sample { ug_m3: 10.0 })
        );
    });
}

#[test]
fn ack_frame_before_data_resyncs_without_failing() {
    block_on(async {
        // A stale command acknowledgement arrives first; the parser
        // rejects it on its length word (not a 28-byte data frame) and
        // resyncs for the real frame.
        let mut pms = Pms5003::new(
            ScriptedUart::new(&[
                ReadStep::Bytes(&COMMAND_ACK),
                ReadStep::Bytes(&CLEAN_FRAME),
            ]),
            MockWait::new(),
        );
        pms.init().await.expect("init succeeds");
        let payload = pms
            .sample()
            .await
            .expect("an interleaved ack is not a sensor fault");
        assert_eq!(
            payload,
            MeasurementPayload::Pm25(Pm25Sample { ug_m3: 10.0 })
        );
    });
}

#[test]
fn checksum_violation_is_a_reading_fault() {
    block_on(async {
        let mut pms = Pms5003::new(
            ScriptedUart::new(&[ReadStep::Bytes(&CORRUPTED_FRAME)]),
            MockWait::new(),
        );
        pms.init().await.expect("init succeeds");
        let error = pms.sample().await.expect_err("a corrupt frame must fault");
        assert_eq!(error.fault, SensorFault::Reading);
        assert_eq!(error.code, 3); // checksum
    });
}

#[test]
fn sensor_error_bits_are_a_hardware_fault() {
    block_on(async {
        // Version 100, error bits 0x05: the sensor reports its own fault.
        let mut pms = Pms5003::new(
            ScriptedUart::new(&[ReadStep::Bytes(&FAULTY_FRAME)]),
            MockWait::new(),
        );
        pms.init().await.expect("init succeeds");
        let error = pms.sample().await.expect_err("error bits must fault");
        assert_eq!(error.fault, SensorFault::Hardware);
        assert_eq!(error.code, 5); // the sensor's own code rides through
    });
}

#[test]
fn silent_sensor_is_a_bus_fault() {
    block_on(async {
        let mut pms = Pms5003::new(
            ScriptedUart::new(&[ReadStep::Fail(UartError::Timeout)]),
            MockWait::new(),
        );
        pms.init().await.expect("init succeeds");
        let error = pms.sample().await.expect_err("silence must fault");
        assert_eq!(error.fault, SensorFault::Bus);
        assert_eq!(error.code, 1); // read
    });
}

#[test]
fn a_faulted_sample_recovers_on_the_next_one() {
    block_on(async {
        // One line-noise read, then a clean frame: the parser state is
        // discarded with the fault, so the next sample publishes normally.
        let mut pms = Pms5003::new(
            ScriptedUart::new(&[
                ReadStep::Fail(UartError::LineNoise),
                ReadStep::Bytes(&CLEAN_FRAME),
            ]),
            MockWait::new(),
        );
        pms.init().await.expect("init succeeds");
        let first = pms.sample().await.expect_err("the noise read faults");
        assert_eq!(first.fault, SensorFault::Bus);
        let second = pms
            .sample()
            .await
            .expect("the next sample recovers without re-init");
        assert_eq!(second, MeasurementPayload::Pm25(Pm25Sample { ug_m3: 10.0 }));
    });
}
