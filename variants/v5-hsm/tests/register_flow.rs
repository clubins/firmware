//! Register-flow conformance: the real BME280 and SHT4x driver machines,
//! driven through the central dispatcher — the architecture's single
//! execution path — against the same recorded I2C transaction sequences
//! the unit tests use.
//!
//! Observation at this layer is intentionally indirect: the dispatcher
//! hands the machines `Box<dyn Machine>` boundaries, so outcomes are read
//! from (a) each `SharedBus`'s consumption log — `expect_done` proves the
//! exact recorded transactions happened, in order, and nothing extra —
//! and (b) the dispatcher's own queue accounting. Reading values and
//! state fields is the unit tests' job.

use firmware_v5_hsm::dispatcher::Dispatcher;
use firmware_v5_hsm::events::{Event, SIG_ENGAGE, SIG_TICK};
use firmware_v5_hsm::sensors::bme280::{self, SIG_BME280_CONVERT};
use firmware_v5_hsm::sensors::bus::{SharedBus, SharedClock, Step};
use firmware_v5_hsm::sensors::sht4x::{self, SIG_SHT4X_MEASURE};

/// BME280 bring-up + one forced conversion, verbatim from the datasheet
/// flow (see `sensors/bme280.rs` for the register rationale).
fn bme280_full_cycle() -> Vec<Step> {
    vec![
        // bring-up: chip id, 26-byte calibration, 7 humidity words,
        // then ctrl_hum and config before any conversion can be triggered
        Step::WriteRead(0x76, vec![0xD0], vec![0x60]),
        Step::WriteRead(0x76, vec![0x88], (0..26).collect()),
        Step::WriteRead(0x76, vec![0xE1], vec![0x36, 0x67, 0, 0, 0, 0, 0]),
        Step::Write(0x76, vec![0xF2, 0x01]), // ctrl_hum: humidity ×1
        Step::Write(0x76, vec![0xF5, 0x00]), // config
        // conversion: trigger, status (done), 8-byte data burst
        Step::Write(0x76, vec![0xF4, 0x25]),
        Step::WriteRead(0x76, vec![0xF3], vec![0x00]),
        Step::WriteRead(
            0x76,
            vec![0xF7],
            vec![0x51, 0xF9, 0x9E, 0x6E, 0x40, 0x7B, 0x6A, 0x92],
        ),
    ]
}

/// SHT4x bring-up (serial read) + one high-precision measurement.
fn sht4x_full_cycle() -> Vec<Step> {
    vec![
        // bring-up: serial number, two CRC-8 protected words
        Step::WriteRead(0x44, vec![0x89], vec![0x12, 0x34, 0x37, 0x56, 0x78, 0x7D]),
        // measurement: single-byte command, then the 6-byte data read
        Step::Write(0x44, vec![0xFD]),
        Step::Read(0x44, vec![0x66, 0x66, 0x93, 0x72, 0xA4, 0x5B]),
    ]
}

/// Bring-up completes on ticks: the BME280 files probe → calibration →
/// ready over two, the SHT4x probe over one; a spare tick is harmless.
fn ticks_bringing_both_up(d: &mut Dispatcher) {
    for _ in 0..3 {
        d.post(Event::new(SIG_TICK));
        d.run_until_idle(128);
    }
}

#[test]
fn both_drivers_bring_up_through_the_dispatcher() {
    let bme_bus = SharedBus::new(bme280_full_cycle()[..5].to_vec());
    let sht_bus = SharedBus::new(sht4x_full_cycle()[..1].to_vec());
    let clock = SharedClock::new(0);

    let mut d = Dispatcher::new(64);
    d.register(Box::new(bme280::driver(
        Box::new(bme_bus.clone()),
        Box::new(clock.clone()),
    )));
    d.register(Box::new(sht4x::driver(
        Box::new(sht_bus.clone()),
        Box::new(clock.clone()),
    )));
    assert_eq!(d.machine_count(), 2);

    d.start_all();
    assert!(d.post(Event::new(SIG_ENGAGE)), "queue has room");
    d.run_until_idle(128);
    ticks_bringing_both_up(&mut d);

    // Every bring-up transaction happened, in order, through the
    // dispatcher-driven machines.
    bme_bus.expect_done();
    sht_bus.expect_done();
    assert!(d.is_empty(), "queue drained to idle");
}

#[test]
fn a_full_measurement_cycle_flows_through_the_dispatcher() {
    let bme_bus = SharedBus::new(bme280_full_cycle());
    let sht_bus = SharedBus::new(sht4x_full_cycle());
    let clock = SharedClock::new(0);

    let mut d = Dispatcher::new(64);
    d.register(Box::new(bme280::driver(
        Box::new(bme_bus.clone()),
        Box::new(clock.clone()),
    )));
    d.register(Box::new(sht4x::driver(
        Box::new(sht_bus.clone()),
        Box::new(clock.clone()),
    )));

    d.start_all();
    d.post(Event::new(SIG_ENGAGE));
    d.run_until_idle(128);
    ticks_bringing_both_up(&mut d);

    // Both cadence signals in one batch: the sensor priority band (12, 13)
    // orders delivery, and both machines run their flows.
    d.post(Event::new(SIG_BME280_CONVERT));
    d.post(Event::new(SIG_SHT4X_MEASURE));
    d.run_until_idle(128);

    // The datasheet's maximum measurement times elapse; the shared tick
    // then completes both machines in the same dispatch pass.
    clock.advance(10);
    d.post(Event::new(SIG_TICK));
    d.run_until_idle(128);

    bme_bus.expect_done();
    sht_bus.expect_done();
    assert!(d.is_empty());
}

#[test]
fn a_sensor_bus_failure_does_not_stall_the_other_machine() {
    // The BME280's probe fails once; bring-up follows for the recovery
    // engage (a fresh probe re-runs the recorded transactions).
    let bme_script = [Step::Fail(firmware_v5_hsm::sensors::bus::BusError::Nack)]
        .into_iter()
        .chain(bme280_full_cycle()[..5].iter().cloned())
        .collect::<Vec<_>>();
    let bme_bus = SharedBus::new(bme_script);
    let sht_bus = SharedBus::new(sht4x_full_cycle());
    let clock = SharedClock::new(0);

    let mut d = Dispatcher::new(64);
    d.register(Box::new(bme280::driver(
        Box::new(bme_bus.clone()),
        Box::new(clock.clone()),
    )));
    d.register(Box::new(sht4x::driver(
        Box::new(sht_bus.clone()),
        Box::new(clock.clone()),
    )));

    d.start_all();
    d.post(Event::new(SIG_ENGAGE));
    d.run_until_idle(128);

    // Only the failed probe was attempted on the BME280 bus; the first
    // tick files the fault without touching the bus again.
    assert_eq!(
        bme_bus.performed().len(),
        1,
        "probe failed, nothing further"
    );
    ticks_bringing_both_up(&mut d);
    assert_eq!(bme_bus.performed().len(), 1, "faulted machine stays parked");

    // One cadence batch: the SHT4x measures through to its data read.
    // The batch is dispatched while the clock still reads 0 — the
    // measurement starts now and its deadline is 9 ms out — and the
    // tick after the advance completes it.
    d.post(Event::new(SIG_BME280_CONVERT));
    d.post(Event::new(SIG_SHT4X_MEASURE));
    d.run_until_idle(128);
    clock.advance(10);
    d.post(Event::new(SIG_TICK));
    d.run_until_idle(128);
    sht_bus.expect_done();
    assert_eq!(bme_bus.performed().len(), 1);

    // Re-engage (broadcast): the BME280 retries bring-up from the top and
    // completes its recorded sequence; the SHT4x treats it as idempotent.
    d.post(Event::new(SIG_ENGAGE));
    d.run_until_idle(128);
    ticks_bringing_both_up(&mut d);
    bme_bus.expect_done();
    assert!(d.is_empty());
}
