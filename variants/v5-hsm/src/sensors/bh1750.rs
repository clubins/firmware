//! BH1750 driver — gated by the `sensor-bh1750` flag.
//!
//! I2C ambient light → emitted as the firmware-core [`LightReading`] (the
//! `light` ingest type, spec §Sensor Expansion: new measurement type,
//! one-topic-per-type). Lux is the only quantity this sensor has, and 0 lx
//! is an honest answer — complete darkness, not a failure marker (the
//! SCD4x's zero-CO₂ rule does not transfer: that sensor reserves 0 for
//! "injected after measurement failed"; the BH1750 has no such marker and
//! no checksum either — a transferred count word is a count word).
//!
//! The register flow is the datasheet's continuous H-resolution cycle
//! (BH1750FVI datasheet; the address pin low fixes the bus address at
//! 0x23): power on (0x01), configure the measurement time (MTreg — two
//! one-byte commands, `0x40+MTreg[7:5]` then `0x60+MTreg[4:0]`, always
//! written so the flow is deterministic regardless of the power-on
//! default), then start continuous H-res mode (0x10). From then on the
//! data register refreshes itself once per conversion window and each
//! cadence post is answered by a plain 2-byte read — no command, no
//! ready flag. The integration window is 120 ms × MTreg/69 (typ, max
//! 180 ms × MTreg/69), so a larger MTreg trades latency for resolution;
//! the lux conversion scales the other way — a longer window accumulates
//! proportionally more counts, so the lux per count falls by 69/MTreg.
//!
//! Pacing is event-driven — the machine never blocks: after the mode
//! starts it waits out the first conversion window (max time, one
//! deadline on `SIG_TICK`) before reporting ready, because the data
//! register's first refresh only lands then. The stub machine at the
//! bottom of this file remains the composition-root placeholder until
//! the esp-hal I2C adapter lands (docs/plan/build-dag.md,
//! `feat/driver-buses`); [`driver`] builds the real machine over a bus
//! and clock, host-tested against recorded transactions (`ScriptedBus`).

use crate::events::{Event, SIG_ENGAGE, SIG_MEAS_READY, SIG_TICK, Signal};
use crate::fsm::{Ctx, Outcome, StateDef, StateId};
use crate::measurement::LightReading;
use crate::sensors::bus::{Clock, I2cBus};
use crate::stubkit::{self, HsmMachine, StubCtx};
use alloc::boxed::Box;

/// Driver-domain signal: a lux reading is due. The cadence posts it; this
/// machine subscribes.
pub const SIG_BH1750_READ: Signal = Signal::new(160);

/// Dispatcher position: sensors deliver before transports and ops.
pub const PRIORITY: u8 = 16;

/// Signals this machine wants: bring-up, the measurement cadence, and the
/// shared tick for datasheet-timed waits.
pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_BH1750_READ, SIG_TICK];

// Commands (BH1750FVI datasheet; single-byte opcodes on the wire).
const ADDR: u8 = 0x23; // address pin low (the common breakout ties it)
const CMD_POWER_ON: u8 = 0x01;
const CMD_CONT_H_RES: u8 = 0x10;
const DATA_LEN: usize = 2; // one count word, big-endian, no checksum
/// The MTreg the datasheet's 1 lx/120 ms conversion is calibrated for.
const MTREG_DEFAULT: u16 = 69;

/// The sensor's measurement-time register (MTreg): the integration window
/// is 120 ms × MTreg/69 (typ), so a larger MTreg trades latency for
/// resolution. The datasheet's usable range is 31..=254 — below it the
/// window is too short for the internal circuit, above it the encoding
/// overflows — and the type makes those states unrepresentable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasurementTime(u16);

/// Why a raw MTreg was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementTimeError {
    /// Outside the datasheet's usable range 31..=254.
    OutOfRange(u16),
}

impl core::fmt::Display for MeasurementTimeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OutOfRange(raw) => {
                write!(f, "MTreg {raw} outside the usable range 31..=254")
            }
        }
    }
}

impl core::error::Error for MeasurementTimeError {}

impl MeasurementTime {
    /// The datasheet default (69): 120 ms typical, 1 lx resolution.
    pub const DEFAULT: Self = Self(MTREG_DEFAULT);

    /// The lowest usable value (31).
    pub const MIN: u16 = 31;
    /// The highest usable value (254).
    pub const MAX: u16 = 254;

    /// Validates against the datasheet range.
    pub fn new(raw: u16) -> Result<Self, MeasurementTimeError> {
        (Self::MIN..=Self::MAX)
            .contains(&raw)
            .then_some(Self(raw))
            .ok_or(MeasurementTimeError::OutOfRange(raw))
    }

    /// The raw register value.
    pub fn raw(self) -> u16 {
        self.0
    }
}

/// The two measurement-time command bytes (`0x40+MTreg[7:5]`,
/// `0x60+MTreg[4:0]`), written as two one-byte writes in this order.
fn measurement_time_commands(mt: u16) -> [u8; 2] {
    [0x40 | (mt >> 5) as u8, 0x60 | (mt & 0x1F) as u8]
}

/// Lux from raw counts: H-res mode at the default MTreg converts at 1/1.2
/// lx per count, and the integration scales linearly with MTreg — a longer
/// window accumulates proportionally more counts, so the lux per count
/// falls by 69/MTreg (BH1750FVI datasheet, measurement time).
fn counts_to_lux(counts: u16, mt: u16) -> f32 {
    f32::from(counts) / 1.2 * (f32::from(MTREG_DEFAULT) / f32::from(mt))
}

/// The longest first-conversion wait: H-res max 180 ms at the default
/// MTreg, scaling linearly (ceiling to whole ms).
fn max_first_window_ms(mt: u16) -> u64 {
    (180u64 * u64::from(mt) + u64::from(MTREG_DEFAULT) - 1) / u64::from(MTREG_DEFAULT)
}

// ---- the driver machine ----

pub const ROOT: StateId = 0;
pub const ONLINE: StateId = 1;
pub const PROBING: StateId = 2;
pub const CONFIGURING: StateId = 3;
pub const WAITING: StateId = 4;
pub const READY: StateId = 5;
pub const SAMPLING: StateId = 6;
pub const FAULT: StateId = 7;

/// What the last entry action concluded — handlers consult it on the
/// next tick instead of re-reading the bus.
#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Probed,
    Configured,
    Sampled,
    Faulted,
}

/// The BH1750 machine's context: the bus and clock it runs over plus the
/// observable state the system (and tests) read.
pub struct Driver {
    bus: Box<dyn I2cBus>,
    clock: Box<dyn Clock>,
    /// The configured measurement time (MTreg).
    pub mtreg: MeasurementTime,
    /// The most recent reading; set when `SIG_MEAS_READY` posts. 0 lx is
    /// stored as-is — darkness is a reading, not a fault.
    pub reading: Option<LightReading>,
    /// Completed measurements since registration.
    pub measurements: u32,
    /// Failed transactions (bring-up or sampling) since registration.
    pub faults: u32,
    /// The sensor's serial-bus address.
    pub addr: u8,
    phase: Phase,
    /// Earliest ms at which the first-conversion wait ends — the
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

/// Bring-up: power on. The write's ACK is the presence proof (the device
/// answers its address); anything else parks the machine in [`FAULT`]
/// on the next tick.
fn probing_entry(d: &mut Driver, _: &mut Ctx) {
    d.phase = match d.bus.write(d.addr, &[CMD_POWER_ON]) {
        Ok(()) => Phase::Probed,
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
            Phase::Probed => Outcome::Transit(CONFIGURING),
            _ => Outcome::Transit(FAULT),
        },
        _ => Outcome::Unhandled,
    }
}

/// Configure and start: the two MTreg command bytes, then continuous
/// H-res mode. One register flow, no replies.
fn configuring_entry(d: &mut Driver, _: &mut Ctx) {
    let [hi, lo] = measurement_time_commands(d.mtreg.raw());
    let ok = d.bus.write(d.addr, &[hi]).is_ok()
        && d.bus.write(d.addr, &[lo]).is_ok()
        && d.bus.write(d.addr, &[CMD_CONT_H_RES]).is_ok();
    d.phase = if ok {
        Phase::Configured
    } else {
        d.faults += 1;
        Phase::Faulted
    };
}

fn configuring_handler(d: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_TICK => match d.phase {
            Phase::Configured => Outcome::Transit(WAITING),
            _ => Outcome::Transit(FAULT),
        },
        _ => Outcome::Unhandled,
    }
}

/// The first conversion only lands after one full window; the machine
/// waits out the maximum rather than sampling early.
fn waiting_entry(d: &mut Driver, _: &mut Ctx) {
    d.deadline_ms = d.clock.now_ms() + max_first_window_ms(d.mtreg.raw());
}

fn waiting_handler(d: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_TICK => {
            if d.clock.now_ms() >= d.deadline_ms {
                Outcome::Transit(READY)
            } else {
                // Still inside the first conversion window. The event
                // climbs and lands in the unhandled count — visible, and
                // the next cadence post measures for real.
                Outcome::Unhandled
            }
        }
        // A cadence post during the first window arrives before any
        // conversion exists; let it propagate (counted) rather than
        // swallow it — the skipped measurement stays observable.
        _ => Outcome::Unhandled,
    }
}

fn ready_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_BH1750_READ => Outcome::Transit(SAMPLING),
        _ => Outcome::Unhandled,
    }
}

/// One cadence post: read the count word (continuous mode needs no
/// command), convert, and post — all in the entry action, with the
/// outcome filed on the next tick (initial transitions may only descend).
fn sampling_entry(d: &mut Driver, out: &mut Ctx) {
    let mut reply = [0u8; DATA_LEN];
    d.phase = match d.bus.read(d.addr, &mut reply) {
        Ok(()) => {
            let counts = u16::from(reply[0]) << 8 | u16::from(reply[1]);
            d.reading = Some(LightReading {
                lux: counts_to_lux(counts, d.mtreg.raw()),
            });
            d.measurements += 1;
            out.post(Event::new(SIG_MEAS_READY));
            Phase::Sampled
        }
        Err(_) => {
            d.faults += 1;
            Phase::Faulted
        }
    };
}

fn sampling_handler(_d: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_TICK => Outcome::Transit(READY),
        // Busy: repeated cadence posts are idempotent while sampling.
        s if s == SIG_BH1750_READ => Outcome::Handled,
        _ => Outcome::Unhandled,
    }
}

fn fault_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        s if s == SIG_ENGAGE => Outcome::Transit(PROBING),
        _ => Outcome::Unhandled,
    }
}

static BH1750_TABLE: &[StateDef<Driver>] = &[
    StateDef {
        name: "bh1750:root",
        parent: None,
        entry: None,
        exit: None,
        handler: root_handler,
        transitions: &[],
    },
    StateDef {
        name: "bh1750:online",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: online_handler,
        transitions: &[],
    },
    StateDef {
        name: "bh1750:probing",
        parent: Some(ONLINE),
        entry: Some(probing_entry),
        exit: None,
        handler: probing_handler,
        transitions: &[],
    },
    StateDef {
        name: "bh1750:configuring",
        parent: Some(ONLINE),
        entry: Some(configuring_entry),
        exit: None,
        handler: configuring_handler,
        transitions: &[],
    },
    StateDef {
        name: "bh1750:waiting",
        parent: Some(ONLINE),
        entry: Some(waiting_entry),
        exit: None,
        handler: waiting_handler,
        transitions: &[],
    },
    StateDef {
        name: "bh1750:ready",
        parent: Some(ONLINE),
        entry: None,
        exit: None,
        handler: ready_handler,
        transitions: &[],
    },
    StateDef {
        name: "bh1750:sampling",
        parent: Some(ONLINE),
        entry: Some(sampling_entry),
        exit: None,
        handler: sampling_handler,
        transitions: &[],
    },
    StateDef {
        name: "bh1750:fault",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: fault_handler,
        transitions: &[],
    },
];

/// Builds the real BH1750 machine over a bus/clock pair and a validated
/// measurement time. The target-side composition root wires the esp-hal
/// adapter here once it lands; host tests pass `SharedBus` and
/// `SharedClock`.
pub fn driver(
    bus: Box<dyn I2cBus>,
    clock: Box<dyn Clock>,
    mtreg: MeasurementTime,
) -> HsmMachine<Driver> {
    let data = Driver {
        bus,
        clock,
        mtreg,
        reading: None,
        measurements: 0,
        faults: 0,
        addr: ADDR,
        phase: Phase::Probed,
        deadline_ms: 0,
    };
    HsmMachine::new(
        "bh1750",
        PRIORITY,
        SUBSCRIPTIONS,
        BH1750_TABLE,
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
    stubkit::machine("bh1750", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::sensors::bus::{BusError, SharedBus, SharedClock, Step};

    fn bringup_script(mt: u16) -> Vec<Step> {
        let [hi, lo] = measurement_time_commands(mt);
        vec![
            Step::Write(ADDR, vec![CMD_POWER_ON]),
            Step::Write(ADDR, vec![hi]),
            Step::Write(ADDR, vec![lo]),
            Step::Write(ADDR, vec![CMD_CONT_H_RES]),
        ]
    }

    /// Drives the three-tick bring-up (probe, configure, first-window);
    /// the clock handle advances past the first conversion window.
    fn bring_up(m: &mut HsmMachine<Driver>, clock: &SharedClock, out: &mut Ctx) {
        m.handle(&Event::new(SIG_ENGAGE), out);
        m.handle(&Event::new(SIG_TICK), out);
        assert_eq!(m.hsm.current(), CONFIGURING);
        m.handle(&Event::new(SIG_TICK), out);
        assert_eq!(m.hsm.current(), WAITING);
        clock.advance(10_000); // past any first-conversion window
        m.handle(&Event::new(SIG_TICK), out);
        assert_eq!(m.hsm.current(), READY);
    }

    #[test]
    fn measurement_time_validates_the_datasheet_range() {
        assert_eq!(
            MeasurementTime::new(30),
            Err(MeasurementTimeError::OutOfRange(30))
        );
        assert_eq!(
            MeasurementTime::new(255),
            Err(MeasurementTimeError::OutOfRange(255))
        );
        assert_eq!(MeasurementTime::new(31), Ok(MeasurementTime(31)));
        assert_eq!(MeasurementTime::new(254), Ok(MeasurementTime(254)));
        assert_eq!(MeasurementTime::DEFAULT.raw(), MTREG_DEFAULT);
        let err = MeasurementTime::new(0).expect_err("0 is out of range");
        assert!(err.to_string().contains("31..=254"));
    }

    #[test]
    fn measurement_time_encodes_into_the_two_command_bytes() {
        // Default 69 = 0b100_0101: no-op on the high bits, low 5 bits.
        assert_eq!(
            measurement_time_commands(69),
            [0x40 | 0b010, 0x60 | 0b00101]
        );
        // 138 = 0b1000_1010 → 0x44, 0x6A.
        assert_eq!(measurement_time_commands(138), [0x44, 0x6A]);
        // Max 254 = 0b1111_1110 → 0x47, 0x7E.
        assert_eq!(measurement_time_commands(254), [0x47, 0x7E]);
    }

    #[test]
    fn counts_convert_to_lux_scaled_by_mtreg() {
        // Default: 1200 counts → 1000 lx.
        assert!((counts_to_lux(1200, 69) - 1000.0).abs() < 1e-4);
        // Doubled MTreg doubles the counts per lux → half the lux.
        assert!((counts_to_lux(1200, 138) - 500.0).abs() < 1e-4);
        // Darkness is 0 lx, not an error.
        assert_eq!(counts_to_lux(0, 69), 0.0);
    }

    #[test]
    fn first_window_scales_with_mtreg_and_ceils() {
        assert_eq!(max_first_window_ms(69), 180);
        assert_eq!(max_first_window_ms(138), 360);
        assert_eq!(max_first_window_ms(31), 81); // 180×31/69 = 80.87 → 81
    }

    #[test]
    fn bring_up_powers_on_configures_mtreg_and_starts_h_res() {
        let bus = SharedBus::new(bringup_script(69));
        let clock = SharedClock::new(0);
        let mut m = driver(
            Box::new(bus.clone()),
            Box::new(clock.clone()),
            MeasurementTime::DEFAULT,
        );
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        bring_up(&mut m, &clock, &mut out);
        assert_eq!(m.data.faults, 0);
        assert!(m.data.reading.is_none());
        bus.expect_done();
    }

    #[test]
    fn a_failed_probe_parks_in_fault_until_reengaged() {
        let bus = SharedBus::new(
            [Step::Fail(BusError::Nack)]
                .into_iter()
                .chain(bringup_script(69))
                .collect::<Vec<_>>(),
        );
        let clock = SharedClock::new(0);
        let mut m = driver(
            Box::new(bus.clone()),
            Box::new(clock.clone()),
            MeasurementTime::DEFAULT,
        );
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), FAULT);
        assert_eq!(m.data.faults, 1);
        // Re-engage retries bring-up from the top.
        bring_up(&mut m, &clock, &mut out);
        assert_eq!(m.data.faults, 1);
        bus.expect_done();
    }

    #[test]
    fn a_failed_configuration_parks_in_fault_until_reengaged() {
        // Bring-up probes (power on), then the first MTreg command fails;
        // the short-circuit stops the register flow right there.
        let bus = SharedBus::new(
            vec![
                Step::Write(ADDR, vec![CMD_POWER_ON]),
                Step::Fail(BusError::Other),
            ]
            .into_iter()
            .chain(bringup_script(138))
            .collect::<Vec<_>>(),
        );
        let clock = SharedClock::new(0);
        let mut m = driver(
            Box::new(bus.clone()),
            Box::new(clock.clone()),
            MeasurementTime::new(138).expect("138 is in range"),
        );
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), FAULT);
        assert_eq!(m.data.faults, 1);
        // Re-engage retries bring-up from the top.
        bring_up(&mut m, &clock, &mut out);
        assert_eq!(m.data.faults, 1);
        bus.expect_done();
    }

    #[test]
    fn the_first_window_waits_out_the_max_conversion_time() {
        let bus = SharedBus::new(bringup_script(138));
        let clock = SharedClock::new(0);
        let mut m = driver(
            Box::new(bus.clone()),
            Box::new(clock.clone()),
            MeasurementTime::new(138).expect("138 is in range"),
        );
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), WAITING);
        // Inside the 360 ms window the tick does not advance the machine.
        clock.advance(180);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), WAITING);
        // Past the deadline it reports ready.
        clock.advance(180);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        bus.expect_done();
    }

    #[test]
    fn a_cadence_post_samples_the_count_word_and_posts_meas_ready() {
        let bus = SharedBus::new(
            bringup_script(69)
                .into_iter()
                .chain([Step::Read(ADDR, vec![0x04, 0xB0])]) // 1200 counts
                .collect::<Vec<_>>(),
        );
        let clock = SharedClock::new(0);
        let mut m = driver(
            Box::new(bus.clone()),
            Box::new(clock.clone()),
            MeasurementTime::DEFAULT,
        );
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        bring_up(&mut m, &clock, &mut out);
        m.handle(&Event::new(SIG_BH1750_READ), &mut out);
        assert_eq!(m.hsm.current(), SAMPLING);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(
            matches!(m.data.reading, Some(LightReading { lux }) if (lux - 1000.0).abs() < 1e-4),
            "1200 counts at MTreg 69 must read 1000 lx"
        );
        assert_eq!(m.data.measurements, 1);
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].signal, SIG_MEAS_READY);
        bus.expect_done();
    }

    #[test]
    fn a_failed_sampling_counts_a_fault_and_stays_up() {
        let bus = SharedBus::new(
            bringup_script(69)
                .into_iter()
                .chain([Step::Fail(BusError::Nack)])
                .collect::<Vec<_>>(),
        );
        let clock = SharedClock::new(0);
        let mut m = driver(
            Box::new(bus.clone()),
            Box::new(clock.clone()),
            MeasurementTime::DEFAULT,
        );
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        bring_up(&mut m, &clock, &mut out);
        m.handle(&Event::new(SIG_BH1750_READ), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(m.data.reading.is_none());
        assert_eq!(m.data.measurements, 0);
        assert_eq!(m.data.faults, 1);
        assert!(posted.is_empty(), "no SIG_MEAS_READY without a reading");
        bus.expect_done();
    }

    #[test]
    fn zero_counts_is_darkness_not_a_failure() {
        let bus = SharedBus::new(
            bringup_script(69)
                .into_iter()
                .chain([Step::Read(ADDR, vec![0x00, 0x00])])
                .collect::<Vec<_>>(),
        );
        let clock = SharedClock::new(0);
        let mut m = driver(
            Box::new(bus.clone()),
            Box::new(clock.clone()),
            MeasurementTime::DEFAULT,
        );
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        bring_up(&mut m, &clock, &mut out);
        m.handle(&Event::new(SIG_BH1750_READ), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), READY);
        assert!(
            matches!(m.data.reading, Some(LightReading { lux }) if lux == 0.0),
            "0 counts is 0 lx — complete darkness, a real reading"
        );
        assert_eq!(m.data.measurements, 1);
        assert_eq!(m.data.faults, 0);
        assert_eq!(posted.len(), 1);
        bus.expect_done();
    }
}
