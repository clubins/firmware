// The file pins the SCD4x and PMS5003 drivers — it needs both features
// on to name their modules at all.
#![cfg(all(feature = "sensor-scd4x", feature = "sensor-pms5003"))]
//! Driver conformance for the I2C + UART sensor pair: the real SCD4x and
//! PMS5003 driver machines, driven through the central dispatcher — the
//! architecture's single execution path — against the same recorded
//! transactions the unit tests use.
//!
//! Observation at this layer is intentionally indirect: the dispatcher
//! hands the machines `Box<dyn Machine>` boundaries, so outcomes are read
//! from (a) the `SharedBus`'s consumption log and the `SharedUart`'s
//! script-exhaustion and written-command log — `expect_done` proves the
//! exact recorded sequence happened, in order, and nothing extra — and
//! (b) the dispatcher's own queue accounting. Reading values and state
//! fields is the unit tests' job.

use firmware_v5_hsm::dispatcher::Dispatcher;
use firmware_v5_hsm::events::{Event, SIG_ENGAGE, SIG_TICK};
use firmware_v5_hsm::sensors::bus::{SharedBus, SharedClock, Step, crc8};
use firmware_v5_hsm::sensors::pms5003::{FRAME_LEN, SIG_PMS5003_FRAME};
use firmware_v5_hsm::sensors::scd4x::SIG_SCD4X_MEASURE;
use firmware_v5_hsm::sensors::uart::{SharedUart, UartStep};
use firmware_v5_hsm::sensors::{pms5003, scd4x};

const SCD_ADDR: u8 = 0x62;

/// SCD4x bring-up (serial probe → start periodic) plus one complete
/// data-ready poll and measurement read, verbatim from the datasheet
/// flow (see `sensors/scd4x.rs` for the command rationale).
fn scd4x_full_cycle() -> Vec<Step> {
    let serial: Vec<u8> = [[0x01u8, 0x23], [0x45, 0x67], [0x89, 0xAB]]
        .iter()
        .flat_map(|w| [w[0], w[1], crc8(w)])
        .collect();
    let data: Vec<u8> = [[0x02u8, 0xF2], [0x66, 0x66], [0x80, 0x00]]
        .iter()
        .flat_map(|w| [w[0], w[1], crc8(w)])
        .collect();
    let ready = vec![0x00, 0x00, crc8(&[0x00, 0x00])];
    vec![
        Step::WriteRead(SCD_ADDR, vec![0x36, 0x83], serial), // probe
        Step::Write(SCD_ADDR, vec![0x21, 0xB1]),             // start periodic
        Step::WriteRead(SCD_ADDR, vec![0xE4, 0xB8], ready),  // data-ready
        Step::WriteRead(SCD_ADDR, vec![0xEC, 0x05], data),   // measurement
    ]
}

/// The 32-byte PMS5003 frame for the given concentrations, checksum
/// included (16-bit sum of the first 30 bytes).
fn frame(pm: (u16, u16, u16)) -> [u8; FRAME_LEN] {
    let mut f = [0u8; FRAME_LEN];
    f[0] = 0x42;
    f[1] = 0x4D;
    f[2..4].copy_from_slice(&28u16.to_be_bytes());
    for (i, w) in [pm.0, pm.1, pm.2].iter().enumerate() {
        f[10 + i * 2..12 + i * 2].copy_from_slice(&w.to_be_bytes());
    }
    let sum: u16 = f[..30].iter().map(|b| u16::from(*b)).sum();
    f[30..32].copy_from_slice(&sum.to_be_bytes());
    f
}

/// The PMS5003 script for a full cycle: the passive-mode command on
/// bring-up, the read request on the cadence, then the reply frame.
fn pms_full_cycle(f: &[u8; FRAME_LEN]) -> Vec<UartStep> {
    vec![
        UartStep::Write(vec![0x42, 0x4D, 0xE1, 0x00, 0x00, 0x01, 0x70]),
        UartStep::Write(vec![0x42, 0x4D, 0xE2, 0x00, 0x00, 0x01, 0x71]),
        UartStep::Read(Vec::from(&f[..])),
    ]
}

/// Bring-up completes on ticks: the SCD4x files probe → start over two,
/// the PMS500x probe over one; a spare tick is harmless.
fn ticks_bringing_both_up(d: &mut Dispatcher) {
    for _ in 0..3 {
        d.post(Event::new(SIG_TICK));
        d.run_until_idle(128);
    }
}

#[test]
fn both_uart_and_i2c_drivers_bring_up_through_the_dispatcher() {
    // Bring-up only: the first two steps of the SCD4x cycle, the passive
    // command alone on the UART.
    let scd_bus = SharedBus::new(scd4x_full_cycle()[..2].to_vec());
    let uart = SharedUart::new(pms_full_cycle(&frame((0, 0, 0)))[..1].to_vec());
    let clock = SharedClock::new(0);

    let mut d = Dispatcher::new(64);
    d.register(Box::new(scd4x::driver(
        Box::new(scd_bus.clone()),
        Box::new(clock.clone()),
    )));
    d.register(Box::new(pms5003::driver(
        Box::new(uart.clone()),
        Box::new(clock.clone()),
    )));
    assert_eq!(d.machine_count(), 2);

    d.start_all();
    assert!(d.post(Event::new(SIG_ENGAGE)), "queue has room");
    d.run_until_idle(128);
    ticks_bringing_both_up(&mut d);

    scd_bus.expect_done();
    uart.expect_done();
    assert_eq!(
        uart.written().len(),
        1,
        "only the passive-mode command so far"
    );
    assert!(d.is_empty(), "queue drained to idle");
}

#[test]
fn both_drivers_complete_a_measurement_cycle_through_the_dispatcher() {
    let scd_bus = SharedBus::new(scd4x_full_cycle());
    let f = frame((12, 34, 56));
    let uart = SharedUart::new(pms_full_cycle(&f));
    let clock = SharedClock::new(0);

    let mut d = Dispatcher::new(64);
    d.register(Box::new(scd4x::driver(
        Box::new(scd_bus.clone()),
        Box::new(clock.clone()),
    )));
    d.register(Box::new(pms5003::driver(
        Box::new(uart.clone()),
        Box::new(clock.clone()),
    )));

    d.start_all();
    d.post(Event::new(SIG_ENGAGE));
    d.run_until_idle(128);
    ticks_bringing_both_up(&mut d);

    // Both cadence signals in one batch: the sensor priority band orders
    // delivery, and both machines enter their polling states.
    d.post(Event::new(SIG_SCD4X_MEASURE));
    d.post(Event::new(SIG_PMS5003_FRAME));
    d.run_until_idle(128);

    // One tick each: the SCD4x finds the data-ready flag set and reads
    // the measurement; the PMS5003 drains the frame in one go. Neither
    // needs the clock to advance — both scripts answer immediately.
    d.post(Event::new(SIG_TICK));
    d.run_until_idle(128);

    scd_bus.expect_done();
    uart.expect_done();
    assert_eq!(
        uart.written().len(),
        2,
        "passive-mode plus the read request"
    );
    assert!(d.is_empty());
}

#[test]
fn a_failed_uart_command_does_not_stall_the_i2c_machine() {
    // The PMS5003's passive-mode write fails; the SCD4x runs its full
    // recorded cycle undisturbed in the same dispatcher.
    let scd_bus = SharedBus::new(scd4x_full_cycle());
    let uart = SharedUart::new([UartStep::Fail(
        firmware_v5_hsm::sensors::uart::UartError::Other,
    )]);
    let clock = SharedClock::new(0);

    let mut d = Dispatcher::new(64);
    d.register(Box::new(scd4x::driver(
        Box::new(scd_bus.clone()),
        Box::new(clock.clone()),
    )));
    d.register(Box::new(pms5003::driver(
        Box::new(uart.clone()),
        Box::new(clock.clone()),
    )));

    d.start_all();
    d.post(Event::new(SIG_ENGAGE));
    d.run_until_idle(128);
    ticks_bringing_both_up(&mut d);

    // The SCD4x completed its full cycle; the PMS5003 attempted only the
    // failed passive-mode command and sits faulted.
    d.post(Event::new(SIG_SCD4X_MEASURE));
    d.run_until_idle(128);
    d.post(Event::new(SIG_TICK));
    d.run_until_idle(128);
    scd_bus.expect_done();
    assert_eq!(
        uart.written().len(),
        1,
        "the failed attempt is logged, nothing more"
    );
    assert!(d.is_empty());
}
