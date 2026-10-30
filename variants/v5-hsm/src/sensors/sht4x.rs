//! SHT4x driver — gated by the `sensor-sht4x` flag.
//!
//! I2C temperature and humidity → emitted as the firmware-core
//! [`TemperatureHumidityReading`]. The SHT4x measures no pressure, so it
//! never fabricates one; a weather post composes its pressure from the
//! BME280.
//!
//! The register flow is the datasheet's single-shot cycle (Sensirion SHT4x
//! datasheet v7.x, commands verified against Sensirion's `embedded-sht`
//! reference driver): read the serial number (0x89, 6 bytes = two CRC-8
//! protected words) as the presence probe, then per measurement: send the
//! high-precision command (0xFD — one byte), wait its maximum duration
//! (8.3 ms per the datasheet), and read the 6 data bytes. Every word is
//! CRC-8 validated (polynomial 0x31, init 0xFF). Conversion follows
//! datasheet equations (1) and (3); the humidity clamps to 0..100 as the
//! datasheet directs for values outside the valid range.
//!
//! Pacing is event-driven — the machine never blocks: the measuring state
//! waits for `SIG_TICK` past the maximum duration. A soft reset (0x94) is
//! deliberately not part of bring-up: the datasheet marks it optional, and
//! the serial-number read already verifies the device answers. The stub
//! machine at the bottom of this file remains the composition-root
//! placeholder until the esp-hal I2C adapter lands
//! (docs/plan/build-dag.md, `feat/driver-buses`); [`driver`] builds the
//! real machine over a bus and clock, host-tested against recorded
//! transactions (`ScriptedBus`).

use crate::events::{Event, SIG_ENGAGE, SIG_MEAS_READY, SIG_TICK, Signal};
use crate::fsm::{Ctx, Outcome, StateDef, StateId};
use crate::measurement::TemperatureHumidityReading;
use crate::sensors::bus::{Clock, I2cBus};
use crate::stubkit::{self, HsmMachine, StubCtx};
use alloc::boxed::Box;

/// Driver-domain signal: a measurement is due. The cadence posts it; this
/// machine subscribes.
pub const SIG_SHT4X_MEASURE: Signal = Signal::new(130);

/// Dispatcher position: sensors deliver before transports and ops.
pub const PRIORITY: u8 = 13;

/// Signals this machine wants: bring-up, the measurement cadence, and the
/// shared tick for datasheet-timed waits.
pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_SHT4X_MEASURE, SIG_TICK];

// Commands and timing (Sensirion SHT4x datasheet; single-byte commands).
const ADDR: u8 = 0x44; // ADDR pin low — the board's default
const CMD_MEAS_HIGH: u8 = 0xFD; // high precision, 8.3 ms max
const CMD_READ_SERIAL: u8 = 0x89;
const REPLY_LEN: usize = 6; // word (2) + CRC, word (2) + CRC
/// Maximum high-precision measurement duration, rounded up to whole ms.
const MEAS_HIGH_MAX_MS: u64 = 9;

/// Sensirion's CRC-8 — one shared definition in the bus module, re-exported
/// so the driver's checksum API (and its pinning tests) read as its own.
pub use crate::sensors::bus::crc8;

/// Validates a two-word CRC-8-protected reply and returns the words.
fn parse_reply(bytes: &[u8; REPLY_LEN]) -> Option<(u16, u16)> {
    let word = |hi: u8, lo: u8| u16::from(hi) << 8 | u16::from(lo);
    let (t, rh) = (word(bytes[0], bytes[1]), word(bytes[3], bytes[4]));
    (crc8(&bytes[0..2]) == bytes[2] && crc8(&bytes[3..5]) == bytes[5]).then_some((t, rh))
}

/// Datasheet equation (1): temperature in °C from raw ticks.
pub fn ticks_to_celsius(raw: u16) -> f32 {
    -45.0 + 175.0 * f32::from(raw) / 65535.0
}

/// Datasheet equation (3): relative humidity in %RH from raw ticks,
/// clamped to the sensor's valid 0..100 range.
pub fn ticks_to_humidity(raw: u16) -> f32 {
    (-6.0 + 125.0 * f32::from(raw) / 65535.0).clamp(0.0, 100.0)
}

// ---- the driver machine ----

pub const ROOT: StateId = 0;
pub const ONLINE: StateId = 1;
pub const PROBING: StateId = 2;
pub const READY: StateId = 3;
pub const MEASURING: StateId = 4;
pub const FAULT: StateId = 5;

/// What the last entry action concluded — handlers consult it on the
/// next tick instead of re-reading the bus.
#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Probed,
    Faulted,
}

/// The SHT4x machine's context: the bus and clock it runs over plus the
/// observable state the system (and tests) read.
pub struct Driver {
    bus: Box<dyn I2cBus>,
    clock: Box<dyn Clock>,
    /// Serial number read at bring-up (both words CRC-validated).
    pub serial: Option<u32>,
    /// The most recent reading; set when `SIG_MEAS_READY` posts.
    pub reading: Option<TemperatureHumidityReading>,
    /// Completed measurements since registration.
    pub measurements: u32,
    /// Failed transactions (bring-up or measurement) since registration.
    pub faults: u32,
    /// The sensor's serial-bus address.
    pub addr: u8,
    phase: Phase,
    abort: bool,
    /// Earliest ms at which the pending measurement may conclude — the
    /// event-driven stand-in for `delay_ms`.
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

/// Bring-up: read the serial number. Presence plus two valid CRCs prove
/// the device answers; anything else parks the machine in [`FAULT`] on
/// the next tick.
fn probing_entry(d: &mut Driver, _: &mut Ctx) {
    let mut reply = [0u8; REPLY_LEN];
    d.phase = match d.bus.write_read(d.addr, &[CMD_READ_SERIAL], &mut reply) {
        Ok(()) => match parse_reply(&reply) {
            Some((w0, w1)) => {
                d.serial = Some(u32::from(w0) << 16 | u32::from(w1));
                Phase::Probed
            }
            None => {
                d.faults += 1;
                Phase::Faulted
            }
        },
        Err(_) => {
            d.faults += 1;
            Phase::Faulted
        }
    };
}

/// Bring-up runs in the entry action; this handler only files the last
/// outcome when the next tick arrives — initial transitions may only
/// descend, so a probe result cannot hop sideways on `INIT`.
fn probing_handler(d: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled, // mid-probe engage is a no-op
        s if s == SIG_TICK => match d.phase {
            Phase::Probed => Outcome::Transit(READY),
            _ => Outcome::Transit(FAULT),
        },
        _ => Outcome::Unhandled,
    }
}

fn ready_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_SHT4X_MEASURE => Outcome::Transit(MEASURING),
        _ => Outcome::Unhandled,
    }
}

/// One single-shot measurement: command, wait the maximum duration, read.
fn measuring_entry(d: &mut Driver, _: &mut Ctx) {
    d.abort = false;
    if d.bus.write(d.addr, &[CMD_MEAS_HIGH]).is_ok() {
        d.resume_at = d.clock.now_ms() + MEAS_HIGH_MAX_MS;
    } else {
        // Measurement failures are retryable in place — skip to READY and
        // let the next cadence post try again.
        d.faults += 1;
        d.abort = true;
    }
}

fn measuring_handler(d: &mut Driver, out: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_TICK => {
            if d.abort {
                // The command write failed — file the retry on this tick.
                return Outcome::Transit(READY);
            }
            if d.clock.now_ms() < d.resume_at {
                return Outcome::Unhandled; // measurement still within its window
            }
            let mut reply = [0u8; REPLY_LEN];
            match d.bus.read(d.addr, &mut reply) {
                Ok(()) => match parse_reply(&reply) {
                    Some((t, rh)) => {
                        d.reading = Some(TemperatureHumidityReading {
                            temperature_c: ticks_to_celsius(t),
                            humidity_percent: ticks_to_humidity(rh),
                        });
                        d.measurements += 1;
                        out.post(Event::new(SIG_MEAS_READY));
                        Outcome::Transit(READY)
                    }
                    None => {
                        // A bad checksum is a corrupted transfer — drop the
                        // measurement, count the fault, stay up.
                        d.faults += 1;
                        Outcome::Transit(READY)
                    }
                },
                Err(_) => {
                    d.faults += 1;
                    Outcome::Transit(READY)
                }
            }
        }
        // Busy: repeated cadence posts and re-engages are idempotent.
        s if s == SIG_SHT4X_MEASURE || s == SIG_ENGAGE => Outcome::Handled,
        _ => Outcome::Unhandled,
    }
}

fn fault_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_ENGAGE => Outcome::Transit(PROBING),
        _ => Outcome::Unhandled,
    }
}

static SHT4X_TABLE: &[StateDef<Driver>] = &[
    StateDef {
        name: "sht4x:root",
        parent: None,
        entry: None,
        exit: None,
        handler: root_handler,
        transitions: &[],
    },
    StateDef {
        name: "sht4x:online",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: online_handler,
        transitions: &[],
    },
    StateDef {
        name: "sht4x:probing",
        parent: Some(ONLINE),
        entry: Some(probing_entry),
        exit: None,
        handler: probing_handler,
        transitions: &[],
    },
    StateDef {
        name: "sht4x:ready",
        parent: Some(ONLINE),
        entry: None,
        exit: None,
        handler: ready_handler,
        transitions: &[],
    },
    StateDef {
        name: "sht4x:measuring",
        parent: Some(ONLINE),
        entry: Some(measuring_entry),
        exit: None,
        handler: measuring_handler,
        transitions: &[],
    },
    StateDef {
        name: "sht4x:fault",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: fault_handler,
        transitions: &[],
    },
];

/// Builds the real SHT4x machine over a bus/clock pair. The target-side
/// composition root wires the esp-hal adapter here once it lands;
/// host tests pass `SharedBus` and `SharedClock`.
pub fn driver(bus: Box<dyn I2cBus>, clock: Box<dyn Clock>) -> HsmMachine<Driver> {
    let data = Driver {
        bus,
        clock,
        serial: None,
        reading: None,
        measurements: 0,
        faults: 0,
        addr: ADDR,
        phase: Phase::Probed,
        abort: false,
        resume_at: 0,
    };
    HsmMachine::new(
        "sht4x",
        PRIORITY,
        SUBSCRIPTIONS,
        SHT4X_TABLE,
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
    stubkit::machine("sht4x", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::sensors::bus::{BusError, SharedBus, SharedClock, Step};

    /// The serial-number reply for serial 0x12345678: two words, each
    /// CRC-8 protected (0x37 and 0x7D per the Sensirion CRC).
    fn serial_reply() -> Vec<u8> {
        vec![
            0x12,
            0x34,
            crc8(&[0x12, 0x34]),
            0x56,
            0x78,
            crc8(&[0x56, 0x78]),
        ]
    }

    /// A measured reply: T ticks 0x6666 (25.0000 °C), RH ticks 0x72A4
    /// (49.9777 %RH), CRCs 0x93 and 0x5B.
    fn data_reply() -> Vec<u8> {
        vec![0x66, 0x66, 0x93, 0x72, 0xA4, 0x5B]
    }

    fn bringup_script() -> Vec<Step> {
        vec![Step::WriteRead(ADDR, vec![CMD_READ_SERIAL], serial_reply())]
    }

    #[test]
    fn crc8_matches_the_sensirion_check_value() {
        assert_eq!(crc8(&[0xBE, 0xEF]), 0x92);
        assert_eq!(crc8(&[0x12, 0x34]), 0x37);
        assert_eq!(crc8(&[0x56, 0x78]), 0x7D);
    }

    #[test]
    fn parses_and_rejects_crc_protected_replies() {
        let good = data_reply();
        assert_eq!(
            parse_reply(good.as_slice().try_into().unwrap()),
            Some((0x6666, 0x72A4))
        );
        let corrupt_t = vec![0x66, 0x66, 0x00, 0x72, 0xA4, 0x5B];
        assert_eq!(
            parse_reply(corrupt_t.as_slice().try_into().unwrap()),
            None,
            "a wrong temperature CRC must invalidate the reply"
        );
        let corrupt_rh = vec![0x66, 0x66, 0x93, 0x72, 0xA4, 0x00];
        assert_eq!(
            parse_reply(corrupt_rh.as_slice().try_into().unwrap()),
            None,
            "a wrong humidity CRC must invalidate the reply"
        );
    }

    #[test]
    fn converts_datasheet_examples() {
        // Datasheet example: 0x6666 ticks → 25.0 °C / 0x72A4 → 49.9777 %RH
        // (equations (1) and (3)).
        let t = ticks_to_celsius(0x6666);
        let h = ticks_to_humidity(0x72A4);
        assert!((t - 25.0).abs() < 0.01, "{t}");
        assert!((h - 49.9777).abs() < 0.01, "{h}");
        // Out-of-range humidity clamps to the valid band.
        assert_eq!(ticks_to_humidity(0xFFFF), 100.0);
        assert_eq!(ticks_to_humidity(0), 0.0);
    }

    #[test]
    fn bring_up_reads_the_serial_number_then_sits_ready() {
        let bus = SharedBus::new(bringup_script());
        let mut m = driver(Box::new(bus.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        // The probe ran in the entry action; the next tick files it.
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.serial, Some(0x1234_5678));
        assert_eq!(m.data.faults, 0);
        bus.expect_done();
    }

    #[test]
    fn measurement_follows_the_recorded_flow() {
        let bus = SharedBus::new(
            bringup_script()
                .into_iter()
                .chain([
                    Step::Write(ADDR, vec![CMD_MEAS_HIGH]), // single-byte command
                    Step::Read(ADDR, data_reply()),
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
        m.handle(&Event::new(SIG_SHT4X_MEASURE), &mut out);
        assert_eq!(m.hsm.current(), MEASURING);
        // The tick before the 8.3 ms max duration must not read yet.
        clock.advance(5);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), MEASURING);
        clock.advance(4);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        let reading = m.data.reading.expect("measured reading");
        assert!((reading.temperature_c - 25.0).abs() < 0.01);
        assert!((reading.humidity_percent - 49.9777).abs() < 0.01);
        assert_eq!(m.data.measurements, 1);
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].signal, SIG_MEAS_READY);
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
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.serial, Some(0x1234_5678));
        bus.expect_done();
    }

    #[test]
    fn a_corrupt_measurement_drops_the_reading_but_stays_up() {
        let bus = SharedBus::new(
            bringup_script()
                .into_iter()
                .chain([
                    Step::Write(ADDR, vec![CMD_MEAS_HIGH]),
                    Step::Read(ADDR, vec![0x66, 0x66, 0x00, 0x72, 0xA4, 0x5B]), // bad T crc
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
        m.handle(&Event::new(SIG_SHT4X_MEASURE), &mut out);
        clock.advance(9);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(m.data.reading.is_none());
        assert_eq!(m.data.measurements, 0);
        assert_eq!(m.data.faults, 1);
        assert!(posted.is_empty(), "no SIG_MEAS_READY without a reading");
        bus.expect_done();
    }
}
