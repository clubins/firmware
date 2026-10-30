//! PMS5003 driver — gated by the `sensor-pms5003` flag.
//!
//! UART particulate sensor → emitted as the firmware-core
//! [`ParticulateReading`] (the `pm25` ingest type, spec §Sensor
//! Expansion: new measurement type, one-topic-per-type). The driver
//! reads the atmospheric-concentration words (PM1.0 / PM2.5 / PM10.0,
//! µg/m³): the CF=1 words are calibrated for the factory environment,
//! and the sensor has no other assigned line for particulates.
//!
//! The serial flow is the Plantower transport protocol in passive mode
//! (PMS5003 datasheet, UART 9600 8N1; the command set and frame layout
//! below follow it): bring-up sets passive mode (`42 4D E1 00 00 01 70`),
//! then each cadence post requests one measurement (`42 4D E2 00 00 01
//! 71`) and reads the 32-byte reply frame — start pair `0x42 0x4D`, a
//! frame-length field of 2×13+2, thirteen data words, and a checksum
//! that is the 16-bit sum of the frame's first 30 bytes. The last data
//! byte (29) is the sensor's error code: zero is normal, anything else
//! is a hardware failure (laser or fan) the machine parks on, the
//! PMS5003 counterpart of the SCD4x's zero-CO₂ marker.
//!
//! Frames arrive as a byte stream with no framing guarantees: chunks can
//! split a frame anywhere and noise can precede it, so reassembly hunts
//! the start pair and buffers 32 aligned bytes before the frame is
//! validated — a corrupt frame costs a fault, never a reading. Pacing is
//! event-driven — the machine never blocks: the polling state drains the
//! UART on each `SIG_TICK` and gives up only past a deadline of one
//! measurement cycle plus margin (a passive-mode measurement takes up to
//! ~1 s of fan time). The stub machine at the bottom of this file
//! remains the composition-root placeholder until the esp-hal UART
//! adapter lands (docs/plan/build-dag.md, `feat/driver-buses`);
//! [`driver`] builds the real machine over a UART and clock, host-tested
//! against a fake that replays scripted byte chunks.

use crate::events::{Event, SIG_ENGAGE, SIG_MEAS_READY, SIG_TICK, Signal};
use crate::fsm::{Ctx, Outcome, StateDef, StateId};
use crate::measurement::ParticulateReading;
use crate::sensors::clock::Clock;
use crate::sensors::uart::UartIo;
use crate::stubkit::{self, HsmMachine, StubCtx};
use alloc::boxed::Box;

/// Driver-domain signal: a PM frame is due. Consumed by the driver PR.
pub const SIG_PMS5003_FRAME: Signal = Signal::new(150);

/// Dispatcher position: sensors deliver before transports and ops.
pub const PRIORITY: u8 = 15;

pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_PMS5003_FRAME, SIG_TICK];

// ---- the Plantower passive-mode protocol ----

/// Set passive mode — the sensor then answers one frame per request.
/// Command checksum: the low byte of the sum of the first six bytes.
const CMD_PASSIVE: [u8; 7] = [0x42, 0x4D, 0xE1, 0x00, 0x00, 0x01, 0x70];
/// Request one measurement in passive mode.
const CMD_READ: [u8; 7] = [0x42, 0x4D, 0xE2, 0x00, 0x00, 0x01, 0x71];

/// A complete PMS5003 frame: start pair, length field, 13 data words
/// (26 bytes), and a 2-byte checksum.
pub const FRAME_LEN: usize = 32;
const START: [u8; 2] = [0x42, 0x4D];
/// Bytes 3-4 carry 2×13+2 — the data plus checksum byte count.
const FRAME_LENGTH_FIELD: u16 = 28;
/// The measurement cycle takes up to ~1 s of fan time in passive mode;
/// one cadence interval plus margin bounds the wait per request.
const FRAME_WINDOW_MS: u64 = 2_000;
/// How many UART bytes one drain accepts — comfortably more than a frame
/// so a chunk with leading noise still fits.
const READ_BUF_LEN: usize = 64;

/// Why a delivered frame did not become a reading.
#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    /// The frame's own integrity failed (start pair, length field, or
    /// checksum) — a corrupted transfer, retried on the next cadence.
    BadFrame,
    /// The sensor reports a hardware error in its error-code byte —
    /// the machine parks until re-engaged.
    SensorError,
}

/// The Plantower frame checksum: the 16-bit sum of the frame's first 30
/// bytes.
fn checksum(frame: &[u8]) -> u16 {
    frame[..FRAME_LEN - 2].iter().map(|&b| u16::from(b)).sum()
}

fn word(bytes: &[u8]) -> u16 {
    u16::from(bytes[0]) << 8 | u16::from(bytes[1])
}

/// Parses a complete 32-byte frame (start pair included) into the
/// atmospheric concentrations.
fn parse_frame(frame: &[u8; FRAME_LEN]) -> Result<ParticulateReading, ParseError> {
    let integrity_ok = frame[0] == START[0]
        && frame[1] == START[1]
        && word(&frame[2..4]) == FRAME_LENGTH_FIELD
        && checksum(frame) == word(&frame[30..32]);
    if !integrity_ok {
        return Err(ParseError::BadFrame);
    }
    // Byte 29 is the error code: 0 normal, bits for laser/fan failure.
    if frame[29] != 0 {
        return Err(ParseError::SensorError);
    }
    // Data words 4-6 are the atmospheric concentrations (µg/m³):
    // PM1.0, PM2.5, PM10.0.
    Ok(ParticulateReading {
        pm1_0_ugm3: word(&frame[10..12]),
        pm2_5_ugm3: word(&frame[12..14]),
        pm10_0_ugm3: word(&frame[14..16]),
    })
}

/// Byte-stream reassembly: hunts the start pair, buffers one frame's
/// worth of aligned bytes, and hands the frame out once it has fully
/// landed. Validation lives in [`parse_frame`] — reassembly only aligns.
pub struct FrameBuf {
    buf: [u8; FRAME_LEN],
    len: usize,
    /// The byte held while hunting: a lone `0x42` may open the start
    /// pair, so it is remembered across chunks.
    hunt: Option<u8>,
}

impl Default for FrameBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameBuf {
    pub fn new() -> Self {
        Self {
            buf: [0; FRAME_LEN],
            len: 0,
            hunt: None,
        }
    }

    /// Feeds one received chunk into the reassembler; returns a frame
    /// when this chunk completes one (the buffer then restarts for the
    /// next). Chunks may split frames anywhere and carry leading noise.
    pub fn feed(&mut self, chunk: &[u8]) -> Option<[u8; FRAME_LEN]> {
        for &b in chunk {
            if self.len >= 2 {
                // Aligned: collecting the rest of one frame.
                self.buf[self.len] = b;
                self.len += 1;
                if self.len == FRAME_LEN {
                    self.len = 0;
                    return Some(self.buf);
                }
            } else {
                // Hunting: looking for the 0x42 0x4D start pair.
                self.hunt = match (self.hunt.take(), b) {
                    (Some(0x42), 0x4D) => {
                        self.buf[..2].copy_from_slice(&START);
                        self.len = 2;
                        None
                    }
                    (_, 0x42) => Some(0x42),
                    _ => None,
                };
            }
        }
        None
    }
}

// ---- the driver machine ----

pub const ROOT: StateId = 0;
pub const ONLINE: StateId = 1;
pub const PROBING: StateId = 2;
pub const READY: StateId = 3;
pub const POLLING: StateId = 4;
pub const FAULT: StateId = 5;

/// The PMS5003 machine's context: the UART and clock it runs over plus
/// the observable state the system (and tests) read.
pub struct Driver {
    uart: Box<dyn UartIo>,
    clock: Box<dyn Clock>,
    /// The most recent reading; set when `SIG_MEAS_READY` posts.
    pub reading: Option<ParticulateReading>,
    /// Completed measurements since registration.
    pub measurements: u32,
    /// Failed transactions (bring-up, request, or frame) since
    /// registration.
    pub faults: u32,
    /// Earliest ms at which an unfinished frame read times out — the
    /// event-driven stand-in for a blocking wait.
    deadline_ms: u64,
    /// Byte-stream reassembly between cadence posts.
    frames: FrameBuf,
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

/// Bring-up: set passive mode. The command has no reply — the first
/// requested frame proves the link on the next cadence post. A failed
/// set is retryable in place: the sensor stays answerable (active mode
/// streams unsolicited frames).
fn probing_entry(d: &mut Driver, _: &mut Ctx) {
    if d.uart.write(&CMD_PASSIVE).is_err() {
        d.faults += 1;
    }
}

/// Bring-up runs in the entry action; this handler only advances when
/// the next tick arrives — initial transitions may only descend, so
/// bring-up cannot hop sideways on `INIT`.
fn probing_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled, // mid-probe engage is a no-op
        s if s == SIG_TICK => Outcome::Transit(READY),
        _ => Outcome::Unhandled,
    }
}

fn ready_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_PMS5003_FRAME => Outcome::Transit(POLLING),
        _ => Outcome::Unhandled,
    }
}

/// One cadence post: request a measurement, then drain the UART until
/// the frame lands — the passive-mode measurement takes up to ~1 s of
/// fan time, so an empty read is normal inside the deadline.
fn polling_entry(d: &mut Driver, _: &mut Ctx) {
    d.deadline_ms = d.clock.now_ms() + FRAME_WINDOW_MS;
    if d.uart.write(&CMD_READ).is_err() {
        // The request failed, but frames already in flight still drain.
        d.faults += 1;
    }
}

fn polling_handler(d: &mut Driver, out: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_TICK => drain(d, out),
        // Busy: repeated cadence posts and re-engages are idempotent.
        s if s == SIG_PMS5003_FRAME || s == SIG_ENGAGE => Outcome::Handled,
        _ => Outcome::Unhandled,
    }
}

/// Drains whatever the UART holds and, when a frame has fully landed,
/// turns it into a reading or a counted fault.
fn drain(d: &mut Driver, out: &mut Ctx) -> Outcome {
    let mut chunk = [0u8; READ_BUF_LEN];
    let n = match d.uart.read(&mut chunk) {
        Ok(n) => n,
        Err(_) => {
            d.faults += 1;
            return Outcome::Transit(READY);
        }
    };
    let frame = match (n, d.frames.feed(&chunk[..n])) {
        (0, _) | (_, None) if d.clock.now_ms() >= d.deadline_ms => {
            // Still nothing past the deadline: give up; the next cadence
            // post requests a fresh measurement.
            d.faults += 1;
            return Outcome::Transit(READY);
        }
        (_, Some(frame)) => frame,
        _ => return Outcome::Unhandled, // bytes so far, frame not complete
    };
    match parse_frame(&frame) {
        Ok(reading) => {
            d.reading = Some(reading);
            d.measurements += 1;
            out.post(Event::new(SIG_MEAS_READY));
            Outcome::Transit(READY)
        }
        Err(ParseError::BadFrame) => {
            d.faults += 1;
            Outcome::Transit(READY)
        }
        Err(ParseError::SensorError) => {
            d.faults += 1;
            Outcome::Transit(FAULT)
        }
    }
}

fn fault_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_ENGAGE => Outcome::Transit(PROBING),
        _ => Outcome::Unhandled,
    }
}

static PMS5003_TABLE: &[StateDef<Driver>] = &[
    StateDef {
        name: "pms5003:root",
        parent: None,
        entry: None,
        exit: None,
        handler: root_handler,
        transitions: &[],
    },
    StateDef {
        name: "pms5003:online",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: online_handler,
        transitions: &[],
    },
    StateDef {
        name: "pms5003:probing",
        parent: Some(ONLINE),
        entry: Some(probing_entry),
        exit: None,
        handler: probing_handler,
        transitions: &[],
    },
    StateDef {
        name: "pms5003:ready",
        parent: Some(ONLINE),
        entry: None,
        exit: None,
        handler: ready_handler,
        transitions: &[],
    },
    StateDef {
        name: "pms5003:polling",
        parent: Some(ONLINE),
        entry: Some(polling_entry),
        exit: None,
        handler: polling_handler,
        transitions: &[],
    },
    StateDef {
        name: "pms5003:fault",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: fault_handler,
        transitions: &[],
    },
];

/// Builds the real PMS5003 machine over a UART/clock pair. The
/// target-side composition root wires the esp-hal adapter here once it
/// lands; host tests pass the fake UART and `SharedClock`.
pub fn driver(uart: Box<dyn UartIo>, clock: Box<dyn Clock>) -> HsmMachine<Driver> {
    let data = Driver {
        uart,
        clock,
        reading: None,
        measurements: 0,
        faults: 0,
        deadline_ms: 0,
        frames: FrameBuf::new(),
    };
    HsmMachine::new(
        "pms5003",
        PRIORITY,
        SUBSCRIPTIONS,
        PMS5003_TABLE,
        ROOT,
        stubkit::STUB_DEFER_CAP,
        data,
    )
}

// ---- composition-root placeholder ----

/// The stub lifecycle machine the composition root registers until the
/// esp-hal UART adapter exists (see the module doc). Deliberately shared
/// shape: wiring compiles unchanged when the driver takes over.
pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("pms5003", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::sensors::clock::SharedClock;
    use crate::sensors::uart::{SharedUart, UartError, UartStep};
    use alloc::vec::Vec;

    /// The 32-byte frame for the given atmospheric concentrations
    /// (PM1.0, PM2.5, PM10.0), checksum included.
    fn frame(pm: (u16, u16, u16)) -> [u8; FRAME_LEN] {
        let mut f = [0u8; FRAME_LEN];
        f[..2].copy_from_slice(&START);
        f[2..4].copy_from_slice(&FRAME_LENGTH_FIELD.to_be_bytes());
        for (i, w) in [pm.0, pm.1, pm.2].iter().enumerate() {
            f[10 + i * 2..12 + i * 2].copy_from_slice(&w.to_be_bytes());
        }
        let sum = checksum(&f);
        f[30..32].copy_from_slice(&sum.to_be_bytes());
        f
    }

    /// A frame whose payload no longer matches its checksum.
    fn corrupt(mut f: [u8; FRAME_LEN]) -> [u8; FRAME_LEN] {
        f[7] ^= 0xFF;
        f
    }

    /// A frame with the sensor's laser-error code set, checksum valid.
    fn error_frame(mut f: [u8; FRAME_LEN]) -> [u8; FRAME_LEN] {
        f[29] = 0x81; // laser + fan error bits
        let sum = checksum(&f);
        f[30..32].copy_from_slice(&sum.to_be_bytes());
        f
    }

    /// The bring-up script: the passive-mode command, byte-exact.
    fn bring_up_script() -> Vec<UartStep> {
        vec![UartStep::Write(Vec::from(CMD_PASSIVE))]
    }

    /// A machine mid-measurement: brought up, in POLLING with the read
    /// request written and the UART scripted with the read command plus
    /// `chunks` of the reply stream.
    fn polling(chunks: &[&[u8]]) -> (HsmMachine<Driver>, SharedUart) {
        let mut script = vec![
            UartStep::Write(Vec::from(CMD_PASSIVE)),
            UartStep::Write(Vec::from(CMD_READ)),
        ];
        script.extend(chunks.iter().map(|c| UartStep::Read(Vec::from(*c))));
        let uart = SharedUart::new(script);
        let mut m = driver(Box::new(uart.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        m.handle(&Event::new(SIG_PMS5003_FRAME), &mut out);
        assert_eq!(m.hsm.current(), POLLING);
        (m, uart)
    }

    #[test]
    fn bring_up_sets_passive_mode_then_sits_ready() {
        let uart = SharedUart::new(bring_up_script());
        let mut m = driver(Box::new(uart.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        assert_eq!(m.hsm.current(), PROBING);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(uart.written(), vec![Vec::from(CMD_PASSIVE)]);
        uart.expect_done();
    }

    #[test]
    fn measurement_requests_one_frame_and_emits_the_reading() {
        let good = frame((12, 34, 56));
        let (mut m, uart) = polling(&[&good]);
        let mut posted: Vec<Event> = Vec::new();
        let mut out = Ctx {
            post: &mut |e| posted.push(e),
        };
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(
            matches!(
                m.data.reading,
                Some(ParticulateReading {
                    pm1_0_ugm3: 12,
                    pm2_5_ugm3: 34,
                    pm10_0_ugm3: 56
                })
            ),
            "measured particulates must be (12, 34, 56) µg/m³"
        );
        assert_eq!(m.data.measurements, 1);
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].signal, SIG_MEAS_READY);
        assert_eq!(
            uart.written(),
            vec![Vec::from(CMD_PASSIVE), Vec::from(CMD_READ)]
        );
        uart.expect_done();
    }

    #[test]
    fn a_frame_split_across_chunks_still_parses() {
        let good = frame((100, 200, 300));
        let (mut m, uart) = polling(&[&good[..10], &good[10..21], &good[21..]]);
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        // Two drains land partial chunks — the machine stays polling.
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), POLLING);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), POLLING);
        // The third completes the frame.
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.measurements, 1);
        assert_eq!(m.data.faults, 0);
        uart.expect_done();
    }

    #[test]
    fn noise_before_the_start_pair_is_skipped() {
        let good = frame((1, 2, 3));
        let mut noisy = Vec::new();
        noisy.extend_from_slice(&[0x00, 0xFF, 0x42, 0x00]);
        noisy.extend_from_slice(&good);
        let (mut m, uart) = polling(&[&noisy]);
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.measurements, 1);
        assert_eq!(m.data.faults, 0);
        uart.expect_done();
    }

    #[test]
    fn a_corrupt_frame_drops_the_reading_but_stays_up() {
        let bad = corrupt(frame((12, 34, 56)));
        let (mut m, uart) = polling(&[&bad]);
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(m.data.reading.is_none());
        assert_eq!(m.data.measurements, 0);
        assert_eq!(m.data.faults, 1);
        uart.expect_done();
    }

    #[test]
    fn an_error_code_frame_parks_in_fault_until_reengaged() {
        let bad = error_frame(frame((12, 34, 56)));
        // The re-engage after the fault writes the passive command again.
        let uart = SharedUart::new([
            UartStep::Write(Vec::from(CMD_PASSIVE)),
            UartStep::Write(Vec::from(CMD_READ)),
            UartStep::Read(Vec::from(bad)),
            UartStep::Write(Vec::from(CMD_PASSIVE)),
        ]);
        let mut m = driver(Box::new(uart.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_PMS5003_FRAME), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), FAULT);
        assert!(m.data.reading.is_none());
        // Re-engaging re-probes and recovers.
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(uart.written().len(), 3); // passive, read, passive again
        uart.expect_done();
    }

    #[test]
    fn a_missing_frame_times_out_and_retries() {
        let clock = SharedClock::new(0);
        let uart = SharedUart::new([
            UartStep::Write(Vec::from(CMD_PASSIVE)),
            UartStep::Write(Vec::from(CMD_READ)),
        ]);
        let mut m = driver(Box::new(uart.clone()), Box::new(clock.clone()));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        m.handle(&Event::new(SIG_PMS5003_FRAME), &mut out);
        assert_eq!(m.hsm.current(), POLLING);
        // An empty drain inside the deadline stays polling...
        clock.advance(1_000);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), POLLING);
        // ...and one past the deadline gives up with a counted fault.
        clock.advance(FRAME_WINDOW_MS);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.faults, 1);
    }

    #[test]
    fn a_failed_uart_read_counts_a_fault_and_retries() {
        let good = frame((12, 34, 56));
        let mut script = vec![
            UartStep::Write(Vec::from(CMD_PASSIVE)),
            UartStep::Write(Vec::from(CMD_READ)),
            UartStep::Fail(UartError::Other),
            // The retry round re-issues the read request, then gets the
            // frame that waited behind the failure.
            UartStep::Write(Vec::from(CMD_READ)),
            UartStep::Read(Vec::from(good)),
        ];
        let uart = SharedUart::new(script);
        let mut m = driver(Box::new(uart.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_PMS5003_FRAME), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.faults, 1);
        // The next drain still reads the frame that waited behind the
        // failure.
        m.handle(&Event::new(SIG_PMS5003_FRAME), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.data.measurements, 1);
        uart.expect_done();
    }

    #[test]
    fn a_failed_passive_mode_command_counts_a_fault() {
        let uart = SharedUart::new([UartStep::Fail(UartError::Other)]);
        let mut m = driver(Box::new(uart.clone()), Box::new(SharedClock::new(0)));
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        // Retryable in place: the machine still reaches READY.
        assert_eq!(m.hsm.current(), READY);
        assert_eq!(m.data.faults, 1);
    }

    #[test]
    fn parses_and_rejects_frames() {
        let good = frame((0x0123, 0x4567, 0x89AB));
        let parsed = parse_frame(&good);
        assert!(
            matches!(
                parsed,
                Ok(ParticulateReading {
                    pm1_0_ugm3: 0x0123,
                    pm2_5_ugm3: 0x4567,
                    pm10_0_ugm3: 0x89AB
                })
            ),
            "the frame must parse to its own concentrations"
        );
        // Wrong start pair.
        let mut wrong_start = good;
        wrong_start[1] = 0x00;
        assert!(matches!(
            parse_frame(&wrong_start),
            Err(ParseError::BadFrame)
        ));
        // Wrong length field.
        let mut wrong_len = good;
        wrong_len[3] = 0x1D;
        assert!(matches!(parse_frame(&wrong_len), Err(ParseError::BadFrame)));
        // Wrong checksum.
        let wrong_ck = corrupt(good);
        assert!(matches!(parse_frame(&wrong_ck), Err(ParseError::BadFrame)));
        // Error code set (checksum recomputed).
        assert!(matches!(
            parse_frame(&error_frame(good)),
            Err(ParseError::SensorError)
        ));
    }

    #[test]
    fn reassembly_hunts_the_pair_across_the_hunt_boundary() {
        let good = frame((0, 0, 0));
        let mut buf = FrameBuf::new();
        // A lone 0x42, then 0x42 0x4D split across chunks, then the rest.
        assert_eq!(buf.feed(&[0x42]), None);
        assert_eq!(buf.feed(&good[..2]), None); // 0x42 0x4D opens the frame
        assert_eq!(buf.feed(&good[2..]), Some(good));
        // The buffer restarts for the next frame.
        assert_eq!(buf.feed(&good), Some(good));
        // Back-to-back frames in one chunk: the feed stops at the first
        // completed frame, and the next drain takes the second.
        let mut both = Vec::new();
        both.extend_from_slice(&good);
        both.extend_from_slice(&good);
        let mut buf = FrameBuf::new();
        assert_eq!(buf.feed(&both), Some(good));
        assert_eq!(buf.feed(&both[FRAME_LEN..]), Some(good));
    }

    #[test]
    fn a_frame_completes_no_matter_how_the_stream_splits() {
        let good = frame((9, 8, 7));
        for s in 0..=FRAME_LEN {
            let mut buf = FrameBuf::new();
            assert_eq!(
                buf.feed(&good[..s]),
                if s == FRAME_LEN { Some(good) } else { None },
                "split at {s}: the first chunk must not complete the frame"
            );
            assert_eq!(
                buf.feed(&good[s..]),
                if s == FRAME_LEN { None } else { Some(good) },
                "split at {s}: the second chunk must finish the frame"
            );
        }
    }
}
