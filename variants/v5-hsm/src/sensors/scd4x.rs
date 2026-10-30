//! SCD4x driver — gated by the `sensor-scd4x` flag.
//!
//! I2C CO₂ → emitted as the firmware-core [`Co2Reading`] (the `co2` ingest
//! type, spec §Sensor Expansion: new measurement type, one-topic-per-type).
//! The sensor's own temperature and humidity words are CRC-validated with
//! the rest of the reply but deliberately dropped: the spec assigns this
//! sensor the CO₂ line only, and the climate line belongs to the BME280 and
//! SHT4x — the model never fabricates a second source for a field.
//!
//! The register flow is the datasheet's periodic-mode cycle (Sensirion
//! SCD4x datasheet and its I2C interface description; every transferred
//! word is CRC-8 protected, Sensirion polynomial 0x31, init 0xFF): read the
//! 48-bit serial number (0x3683 — three words) as the presence probe, start
//! periodic measurement (0x21B1), then per cadence post poll the data-ready
//! status (0xE4B8 — ready when the low 11 bits of the status word clear;
//! the 5 s measurement interval means a poll can legitimately answer "not
//! ready") and, once ready, read the measurement (0xEC05 — CO₂,
//! temperature, humidity words). A CO₂ word of 0 is the datasheet's
//! sensor-failure marker, not a reading; anything else is accepted as-is,
//! the CRC having vouched for the transfer.
//!
//! Pacing is event-driven — the machine never blocks: the polling state
//! checks the flag on each `SIG_TICK` and gives up only past a deadline of
//! one measurement interval plus margin. The stub machine at the bottom of
//! this file remains the composition-root placeholder until the esp-hal
//! I2C adapter lands (docs/plan/build-dag.md, `feat/driver-buses`);
//! [`driver`] builds the real machine over a bus and clock, host-tested
//! against recorded transactions (`ScriptedBus`).

use crate::events::{Event, SIG_ENGAGE, SIG_MEAS_READY, SIG_TICK, Signal};
use crate::fsm::{Ctx, Outcome, StateDef, StateId};
use crate::measurement::Co2Reading;
use crate::sensors::bus::{Clock, I2cBus, crc8};
use crate::stubkit::{self, HsmMachine, StubCtx};
use alloc::boxed::Box;

/// Driver-domain signal: a periodic CO₂ measurement is due. The cadence
/// posts it; this machine subscribes.
pub const SIG_SCD4X_MEASURE: Signal = Signal::new(140);

/// Dispatcher position: sensors deliver before transports and ops.
pub const PRIORITY: u8 = 14;

/// Signals this machine wants: bring-up, the measurement cadence, and the
/// shared tick for datasheet-timed waits.
pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_SCD4X_MEASURE, SIG_TICK];

// Commands and timing (Sensirion SCD4x I2C interface description; 16-bit
// commands, big-endian on the wire).
const ADDR: u8 = 0x62; // fixed I²C address
const CMD_GET_SERIAL: [u8; 2] = [0x36, 0x83]; // → three words + three CRCs
const CMD_START_PERIODIC: [u8; 2] = [0x21, 0xB1];
const CMD_DATA_READY: [u8; 2] = [0xE4, 0xB8]; // → one word + CRC
const CMD_READ_MEAS: [u8; 2] = [0xEC, 0x05]; // → three words + three CRCs
const SERIAL_LEN: usize = 9; // word (2) + CRC, ×3
const MEAS_LEN: usize = 9; // co2, temperature, humidity — each word + CRC
const READY_LEN: usize = 3; // status word + CRC
/// Bits [10:0] of the data-ready word: all clear means a measurement is
/// waiting (`get_data_ready_status`).
const DATA_READY_MASK: u16 = 0x07FF;
/// The datasheet's measurement interval is 5 s with the first reading
/// available ~1 s after start; a poll window of one interval plus margin
/// tolerates a slow first sample without stalling the cadence forever.
const READY_WINDOW_MS: u64 = 6_000;

/// Whether the data-ready status word says a measurement is waiting: the
/// low 11 bits all clear.
pub fn data_ready(word: u16) -> bool {
    word & DATA_READY_MASK == 0
}

/// Validates a one-word CRC-8-protected reply.
fn parse_word(bytes: &[u8; READY_LEN]) -> Option<u16> {
    (crc8(&bytes[0..2]) == bytes[2])
        .then_some(u16::from(bytes[0]) << 8 | u16::from(bytes[1]))
}

/// Validates a three-word CRC-8-protected reply and returns the words.
fn parse_words3(bytes: &[u8; SERIAL_LEN]) -> Option<(u16, u16, u16)> {
    let word = |hi: u8, lo: u8| u16::from(hi) << 8 | u16::from(lo);
    let (w0, w1, w2) = (
        word(bytes[0], bytes[1]),
        word(bytes[3], bytes[4]),
        word(bytes[6], bytes[7]),
    );
    (crc8(&bytes[0..2]) == bytes[2]
        && crc8(&bytes[3..5]) == bytes[5]
        && crc8(&bytes[6..8]) == bytes[8])
        .then_some((w0, w1, w2))
}

// ---- the driver machine ----

pub const ROOT: StateId = 0;
pub const ONLINE: StateId = 1;
pub const PROBING: StateId = 2;
pub const STARTING: StateId = 3;
pub const READY: StateId = 4;
pub const POLLING: StateId = 5;
pub const FAULT: StateId = 6;

/// What the last entry action concluded — handlers consult it on the
/// next tick instead of re-reading the bus.
#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Probed,
    Faulted,
}

/// The SCD4x machine's context: the bus and clock it runs over plus the
/// observable state the system (and tests) read.
pub struct Driver {
    bus: Box<dyn I2cBus>,
    clock: Box<dyn Clock>,
    /// 48-bit serial number read at bring-up (all words CRC-validated).
    pub serial: Option<u64>,
    /// The most recent reading; set when `SIG_MEAS_READY` posts.
    pub reading: Option<Co2Reading>,
    /// Completed measurements since registration.
    pub measurements: u32,
    /// Failed transactions (bring-up, poll, or measurement) since
    /// registration.
    pub faults: u32,
    /// The sensor's serial-bus address.
    pub addr: u8,
    phase: Phase,
    /// Earliest ms at which an unfinished data-ready poll times out — the
    /// event-driven stand-in for a blocking wait.
    deadline_ms: u64,
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

/// Bring-up: read the serial number. Presence plus three valid CRCs prove
/// the device answers; anything else parks the machine in [`FAULT`] on
/// the next tick.
fn probing_entry(d: &mut Driver, _: &mut Ctx) {
    let mut reply = [0u8; SERIAL_LEN];
    d.phase = match d.bus.write_read(d.addr, &CMD_GET_SERIAL, &mut reply) {
        Ok(()) => match parse_words3(&reply) {
            Some((w0, w1, w2)) => {
                d.serial =
                    Some(u64::from(w0) << 32 | u64::from(w1) << 16 | u64::from(w2));
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
            Phase::Probed => Outcome::Transit(STARTING),
            _ => Outcome::Transit(FAULT),
        },
        _ => Outcome::Unhandled,
    }
}

/// Starting the periodic measurement: one command, no reply. The first
/// data-ready flag follows ~1 s later; the cadence polls from [`READY`].
fn starting_entry(d: &mut Driver, _: &mut Ctx) {
    if d.bus.write(d.addr, &CMD_START_PERIODIC).is_err() {
        // Retryable in place — the next cadence post reissues the start.
        d.faults += 1;
    }
}

fn starting_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_TICK => Outcome::Transit(READY),
        _ => Outcome::Unhandled,
    }
}

fn ready_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_SCD4X_MEASURE => Outcome::Transit(POLLING),
        _ => Outcome::Unhandled,
    }
}

/// One cadence post: poll the data-ready flag; when a measurement is
/// waiting, read it. Not-ready answers are normal — the sensor measures
/// every 5 s — so the state keeps polling until its deadline.
fn polling_entry(d: &mut Driver, _: &mut Ctx) {
    d.deadline_ms = d.clock.now_ms() + READY_WINDOW_MS;
}

fn polling_handler(d: &mut Driver, out: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_TICK => {
            let mut status = [0u8; READY_LEN];
            match d.bus.write_read(d.addr, &CMD_DATA_READY, &mut status) {
                Ok(()) => match parse_word(&status) {
                    None => {
                        // A bad checksum is a corrupted transfer — drop the
                        // poll, count the fault, retry on the next cadence.
                        d.faults += 1;
                        Outcome::Transit(READY)
                    }
                    Some(word) if !data_ready(word) => {
                        if d.clock.now_ms() >= d.deadline_ms {
                            d.faults += 1;
                            Outcome::Transit(READY)
                        } else {
                            Outcome::Unhandled // still within the interval
                        }
                    }
                    Some(_) => read_measurement(d, out),
                },
                Err(_) => {
                    d.faults += 1;
                    Outcome::Transit(READY)
                }
            }
        }
        // Busy: repeated cadence posts and re-engages are idempotent.
        s if s == SIG_SCD4X_MEASURE || s == SIG_ENGAGE => Outcome::Handled,
        _ => Outcome::Unhandled,
    }
}

/// Reads the 9-byte measurement. The temperature and humidity words are
/// CRC-validated (a corrupt transfer must not masquerade as a clean one)
/// but dropped — see the module doc.
fn read_measurement(d: &mut Driver, out: &mut Ctx) -> Outcome {
    let mut reply = [0u8; MEAS_LEN];
    let parsed = match d.bus.write_read(d.addr, &CMD_READ_MEAS, &mut reply) {
        Ok(()) => parse_words3(&reply),
        Err(_) => None,
    };
    match parsed {
        // 0 ppm is the datasheet's sensor-failure marker, not a reading.
        Some((co2, _, _)) if co2 != 0 => {
            d.reading = Some(Co2Reading { co2_ppm: co2 });
            d.measurements += 1;
            out.post(Event::new(SIG_MEAS_READY));
            Outcome::Transit(READY)
        }
        _ => {
            d.faults += 1;
            Outcome::Transit(READY)
        }
    }
}

fn fault_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_ENGAGE => Outcome::Transit(PROBING),
        _ => Outcome::Unhandled,
    }
}

static SCD4X_TABLE: &[StateDef<Driver>] = &[
    StateDef {
        name: "scd4x:root",
        parent: None,
        entry: None,
        exit: None,
        handler: root_handler,
        transitions: &[],
    },
    StateDef {
        name: "scd4x:online",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: online_handler,
        transitions: &[],
    },
    StateDef {
        name: "scd4x:probing",
        parent: Some(ONLINE),
        entry: Some(probing_entry),
        exit: None,
        handler: probing_handler,
        transitions: &[],
    },
    StateDef {
        name: "scd4x:starting",
        parent: Some(ONLINE),
        entry: Some(starting_entry),
        exit: None,
        handler: starting_handler,
        transitions: &[],
    },
    StateDef {
        name: "scd4x:ready",
        parent: Some(ONLINE),
        entry: None,
        exit: None,
        handler: ready_handler,
        transitions: &[],
    },
    StateDef {
        name: "scd4x:polling",
        parent: Some(ONLINE),
        entry: Some(polling_entry),
        exit: None,
        handler: polling_handler,
        transitions: &[],
    },
    StateDef {
        name: "scd4x:fault",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: fault_handler,
        transitions: &[],
    },
];

/// Builds the real SCD4x machine over a bus/clock pair. The target-side
/// composition root wires the esp-hal adapter here once it lands; host
/// tests pass `SharedBus` and `SharedClock`.
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
        deadline_ms: 0,
    };
    HsmMachine::new(
        "scd4x",
        PRIORITY,
        SUBSCRIPTIONS,
        SCD4X_TABLE,
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
    stubkit::machine("scd4x", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::sensors::bus::{BusError, SharedBus, SharedClock, Step};

    /// The serial-number reply for serial 0x0123_4567_89AB: three words,
    /// each CRC-8 protected.
    fn serial_reply() -> Vec<u8> {
        let words = [[0x01u8, 0x23], [0x45, 0x67], [0x89, 0xAB]];
        words.iter().flat_map(|w| [w[0], w[1], crc8(w)]).collect()
    }

    /// A measured reply: CO₂ 754 ppm, temperature ticks 0x6666, humidity
    /// ticks 0x8000, each word CRC-protected.
    fn data_reply(co2: u16) -> Vec<u8> {
        let [hi, lo] = co2.to_be_bytes();
        let words = [[hi, lo], [0x66, 0x66], [0x80, 0x00]];
        words.iter().flat_map(|w| [w[0], w[1], crc8(w)]).collect()
    }

    fn ready_reply() -> Vec<u8> {
        vec![0x00, 0x00, crc8(&[0x00, 0x00])]
    }

    fn bringup_script() -> Vec<Step> {
        vec![
            Step::WriteRead(ADDR, CMD_GET_SERIAL.to_vec(), serial_reply()),
            Step::Write(ADDR, CMD_START_PERIODIC.to_vec()),
        ]
    }

    /// Drives the two-tick bring-up (probe files on the first tick, the
    /// start command on the second).
    fn bring_up(m: &mut HsmMachine<Driver>, out: &mut Ctx) {
        m.handle(&Event::new(SIG_ENGAGE), out);
        m.handle(&Event::new(SIG_TICK), out);
        assert_eq!(m.hsm.current(), STARTING);
        m.handle(&Event::new(SIG_TICK), out);
        assert_eq!(m.hsm.current(), READY);
    }

    #[test]
    fn data_ready_is_the_low_eleven_bits() {
        assert!(data_ready(0x0000));
        assert!(data_ready(0x0800)); // bit 11 is outside the ready mask
        assert!(!data_ready(0x0001));
        assert!(!data_ready(0x07FF));
    }

    #[test]
    fn parses_and_rejects_crc_protected_replies() {
        let good = serial_reply();
        assert_eq!(
            parse_words3(good.as_slice().try_into().unwrap()),
            Some((0x0123, 0x4567, 0x89AB))
        );
        let mut corrupt = good.clone();
        corrupt[2] = 0x00; // wrong CRC on the first word
        assert_eq!(
            parse_words3(corrupt.as_slice().try_into().unwrap()),
            None,
            "a wrong serial CRC must invalidate the reply"
        );
        assert_eq!(
            parse_word(ready_reply().as_slice().try_into().unwrap()),
            Some(0x0000)
        );
        let bad = vec![0x00, 0x00, 0xFF];
        assert_eq!(parse_word(bad.as_slice().try_into().unwrap()), None);
    }

    #[test]
    fn bring_up_reads_the_serial_number_then_starts_periodic_mode() {
        let bus = SharedBus::new(bringup_script());
        let mut m = driver(Box::new(bus.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        bring_up(&mut m, &mut out);
        assert_eq!(m.data.serial, Some(0x0123_4567_89AB));
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
        bring_up(&mut m, &mut out);
        assert_eq!(m.data.serial, Some(0x0123_4567_89AB));
        bus.expect_done();
    }

    #[test]
    fn measurement_polls_data_ready_then_reads() {
        let bus = SharedBus::new(
            bringup_script()
                .into_iter()
                .chain([
                    Step::WriteRead(ADDR, CMD_DATA_READY.to_vec(), ready_reply()),
                    Step::WriteRead(ADDR, CMD_READ_MEAS.to_vec(), data_reply(754)),
                ])
                .collect::<Vec<_>>(),
        );
        let mut m = driver(Box::new(bus.clone()), Box::new(SharedClock::new(0)));
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        bring_up(&mut m, &mut out);
        m.handle(&Event::new(SIG_SCD4X_MEASURE), &mut out);
        assert_eq!(m.hsm.current(), POLLING);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(
            matches!(m.data.reading, Some(Co2Reading { co2_ppm: 754 })),
            "measured CO₂ must be 754 ppm"
        );
        assert_eq!(m.data.measurements, 1);
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].signal, SIG_MEAS_READY);
        bus.expect_done();
    }

    #[test]
    fn a_not_ready_poll_waits_then_times_out() {
        let not_ready = vec![0x00, 0x01, crc8(&[0x00, 0x01])];
        let bus = SharedBus::new(
            bringup_script()
                .into_iter()
                .chain([
                    Step::WriteRead(ADDR, CMD_DATA_READY.to_vec(), not_ready.clone()),
                    Step::WriteRead(ADDR, CMD_DATA_READY.to_vec(), not_ready.clone()),
                ])
                .collect::<Vec<_>>(),
        );
        let clock = SharedClock::new(0);
        let mut m = driver(Box::new(bus.clone()), Box::new(clock.clone()));
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        bring_up(&mut m, &mut out);
        m.handle(&Event::new(SIG_SCD4X_MEASURE), &mut out);
        // Inside the window an unanswered poll just keeps polling.
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), POLLING);
        // Past the deadline (one interval plus margin) it files the fault.
        clock.advance(READY_WINDOW_MS);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(m.data.reading.is_none());
        assert_eq!(m.data.faults, 1);
        assert!(posted.is_empty(), "no SIG_MEAS_READY without a reading");
        bus.expect_done();
    }

    #[test]
    fn zero_co2_is_a_sensor_failure_not_a_reading() {
        let bus = SharedBus::new(
            bringup_script()
                .into_iter()
                .chain([
                    Step::WriteRead(ADDR, CMD_DATA_READY.to_vec(), ready_reply()),
                    Step::WriteRead(ADDR, CMD_READ_MEAS.to_vec(), data_reply(0)),
                ])
                .collect::<Vec<_>>(),
        );
        let mut m = driver(Box::new(bus.clone()), Box::new(SharedClock::new(0)));
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        bring_up(&mut m, &mut out);
        m.handle(&Event::new(SIG_SCD4X_MEASURE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(m.data.reading.is_none());
        assert_eq!(m.data.measurements, 0);
        assert_eq!(m.data.faults, 1);
        assert!(posted.is_empty(), "no SIG_MEAS_READY without a reading");
        bus.expect_done();
    }

    #[test]
    fn a_corrupt_measurement_drops_the_reading_but_stays_up() {
        let mut corrupt = data_reply(754);
        corrupt[8] = 0x00; // wrong CRC on the humidity word
        let bus = SharedBus::new(
            bringup_script()
                .into_iter()
                .chain([
                    Step::WriteRead(ADDR, CMD_DATA_READY.to_vec(), ready_reply()),
                    Step::WriteRead(ADDR, CMD_READ_MEAS.to_vec(), corrupt),
                ])
                .collect::<Vec<_>>(),
        );
        let mut m = driver(Box::new(bus.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        bring_up(&mut m, &mut out);
        m.handle(&Event::new(SIG_SCD4X_MEASURE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(m.data.reading.is_none());
        assert_eq!(m.data.measurements, 0);
        assert_eq!(m.data.faults, 1);
        bus.expect_done();
    }
}
