//! PIR occupancy driver — gated by the `sensor-pax` flag.
//!
//! GPIO-interrupt front-end (pin from runtime config) → occupancy events
//! carrying the firmware-core pax measurement's scalar: one debounced
//! motion detection implies occupancy of one ([`OCCUPANCY_PER_MOTION`]),
//! reported honestly as the event it is — never a head-count metric (spec
//! §Sensor Expansion, PIR row). No occupancy *reading* type exists in the
//! measurement model on purpose: the transport layer composes each
//! `SIG_PIR_MOTION` event into a `PaxMeasurement` with
//! `occupancy_count = 1`, the only claim a passive-infrared detector can
//! make.
//!
//! The front-end is the common PIR module shape: a retriggerable output
//! that asserts on detected motion and holds through the module's own
//! hold time, then drops. The line has no data protocol and no failure
//! channel — one bit — so the driver samples levels on the shared tick
//! and the state machine itself tracks the line's last level: `ARMED`
//! means "line was low" (entered only on a low sample), `MOTION` means
//! "line was high" (entered only on a high sample). A rising edge is
//! therefore "sampled high in `ARMED`", and no level bookkeeping is
//! needed beyond that.
//!
//! The debounce window is the task's chatter filter: a PIR re-triggers
//! several times for one person walking past, so a rising edge within
//! `debounce_ms` of the last *accepted* motion is dropped — counted in
//! [`Driver::suppressed`], never silent, but never an occupancy event.
//! The edge still moves the machine into `MOTION` (the line is high and
//! the machine follows the line); only the counting is debounced. The
//! window spans the line's oscillation gaps, so sustained motion reads
//! as one event per window, not one per edge.
//!
//! There is no `FAULT` state: the trait is infallible by construction (a
//! stuck line is a constant level, not an error), and a wedged sensor
//! announces itself as silence — `motions` stops growing — which is the
//! status monitor's observation to make. The stub machine at the bottom
//! of this file remains the composition-root placeholder until the
//! esp-hal GPIO adapter lands (docs/plan/build-dag.md,
//! `feat/driver-buses`); [`driver`] builds the real machine over a GPIO
//! input and clock, host-tested against a scripted line.

use crate::events::{Event, Param, SIG_ENGAGE, SIG_MEAS_READY, SIG_TICK, Signal};
use crate::fsm::{Ctx, Outcome, StateDef, StateId};
use crate::sensors::clock::Clock;
use crate::sensors::gpio::GpioInput;
use crate::stubkit::{self, HsmMachine, StubCtx};
use alloc::boxed::Box;

/// Driver-domain signal: the PIR observed motion — the occupancy event.
/// The param carries the implied occupancy count. Emitted by this
/// machine; the transport layer composes pax measurements from it.
pub const SIG_PIR_MOTION: Signal = Signal::new(110);

/// The occupancy one motion detection implies: a PIR answers "is someone
/// moving here?", not "how many" — so each event reports one person's
/// presence, honestly.
pub const OCCUPANCY_PER_MOTION: u32 = 1;

/// Dispatcher position: sensors deliver before transports and ops.
pub const PRIORITY: u8 = 11;

/// Signals this machine wants: bring-up and the shared tick that paces
/// line sampling. The tick rate bounds edge latency and the debounce
/// window's granularity.
pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_TICK];

// ---- the driver machine ----

pub const ROOT: StateId = 0;
pub const ONLINE: StateId = 1;
pub const ARMING: StateId = 2;
pub const ARMED: StateId = 3;
pub const MOTION: StateId = 4;

/// Whether a rising edge at `now_ms` may count as an occupancy event:
/// either no motion has been accepted yet, or the debounce window has
/// fully passed since the last accepted one.
fn outside_debounce_window(
    last_motion_ms: Option<u64>,
    now_ms: u64,
    window_ms: u64,
) -> bool {
    match last_motion_ms {
        None => true,
        Some(t) => now_ms - t >= window_ms,
    }
}

/// The PIR machine's context: the line and clock it runs over, the
/// configured debounce window, and the observable event accounting the
/// system (and tests) read.
pub struct Driver {
    gpio: Box<dyn GpioInput>,
    clock: Box<dyn Clock>,
    /// Minimum spacing between accepted motion events (ms). Edges inside
    /// the window are dropped (counted, not counted *as motion*).
    pub debounce_ms: u64,
    /// Accepted (debounced) motion events since registration.
    pub motions: u32,
    /// Rising edges dropped by the debounce window since registration.
    pub suppressed: u32,
    /// When the last accepted motion landed (ms clock), if any.
    pub last_motion_ms: Option<u64>,
    /// The arming baseline: the level sampled at bring-up, filed on the
    /// next tick (initial transitions may only descend, so the baseline
    /// cannot hop states on `INIT`).
    baseline: bool,
}

/// A rising edge: accept it as an occupancy event if the debounce window
/// has passed since the last accepted one, then follow the line high.
fn rising_edge(d: &mut Driver, out: &mut Ctx) -> Outcome {
    let now = d.clock.now_ms();
    if outside_debounce_window(d.last_motion_ms, now, d.debounce_ms) {
        d.motions += 1;
        d.last_motion_ms = Some(now);
        out.post(Event::with_param(
            SIG_PIR_MOTION,
            Param::U32(OCCUPANCY_PER_MOTION),
        ));
        out.post(Event::new(SIG_MEAS_READY));
    } else {
        d.suppressed += 1;
    }
    Outcome::Transit(MOTION)
}

fn root_handler(_: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Transit(ARMING),
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

/// Arming: baseline the line so the first armed tick cannot fake an edge
/// from a stale level.
fn arming_entry(d: &mut Driver, _: &mut Ctx) {
    d.baseline = d.gpio.level();
}

fn arming_handler(d: &mut Driver, out: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_TICK => {
            // Bring-up mid-motion is motion: the line is high now, and
            // the first accepted event debounces from here.
            if d.baseline {
                rising_edge(d, out)
            } else {
                Outcome::Transit(ARMED)
            }
        }
        _ => Outcome::Unhandled,
    }
}

/// The line was low: a high sample is a rising edge (debounce-gated), a
/// low sample is nothing new.
fn armed_handler(d: &mut Driver, out: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_TICK => {
            if d.gpio.level() {
                rising_edge(d, out)
            } else {
                Outcome::Handled
            }
        }
        _ => Outcome::Unhandled,
    }
}

/// The line was high (motion detected): a low sample ends the module's
/// hold; a high sample is the hold continuing — already counted.
fn motion_handler(d: &mut Driver, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_ENGAGE => Outcome::Handled,
        s if s == SIG_TICK => {
            if d.gpio.level() {
                Outcome::Handled
            } else {
                Outcome::Transit(ARMED)
            }
        }
        _ => Outcome::Unhandled,
    }
}

static PAX_TABLE: &[StateDef<Driver>] = &[
    StateDef {
        name: "pax:root",
        parent: None,
        entry: None,
        exit: None,
        handler: root_handler,
        transitions: &[],
    },
    StateDef {
        name: "pax:online",
        parent: Some(ROOT),
        entry: None,
        exit: None,
        handler: online_handler,
        transitions: &[],
    },
    StateDef {
        name: "pax:arming",
        parent: Some(ONLINE),
        entry: Some(arming_entry),
        exit: None,
        handler: arming_handler,
        transitions: &[],
    },
    StateDef {
        name: "pax:armed",
        parent: Some(ONLINE),
        entry: None,
        exit: None,
        handler: armed_handler,
        transitions: &[],
    },
    StateDef {
        name: "pax:motion",
        parent: Some(ONLINE),
        entry: None,
        exit: None,
        handler: motion_handler,
        transitions: &[],
    },
];

/// Builds the real PIR machine over a GPIO input, clock, and debounce
/// window. The target-side composition root wires the esp-hal adapter
/// (and the configured pin) here once it lands; host tests pass
/// `SharedGpio` and `SharedClock`. The machine defers nothing — a
/// deferred tick would sample a stale line.
pub fn driver(
    gpio: Box<dyn GpioInput>,
    clock: Box<dyn Clock>,
    debounce_ms: u64,
) -> HsmMachine<Driver> {
    let data = Driver {
        gpio,
        clock,
        debounce_ms,
        motions: 0,
        suppressed: 0,
        last_motion_ms: None,
        baseline: false,
    };
    HsmMachine::new("pax", PRIORITY, SUBSCRIPTIONS, PAX_TABLE, ROOT, 0, data)
}

// ---- composition-root placeholder ----

/// The stub lifecycle machine the composition root registers until the
/// esp-hal GPIO adapter exists (see the module doc). Deliberately shared
/// shape: wiring compiles unchanged when the driver takes over.
pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("pax", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::sensors::clock::SharedClock;
    use crate::sensors::gpio::SharedGpio;

    fn rising(e: &Event) -> bool {
        e.signal == SIG_PIR_MOTION && e.param == Param::U32(OCCUPANCY_PER_MOTION)
    }

    #[test]
    fn debounce_admits_the_first_motion_then_gates_the_window() {
        assert!(outside_debounce_window(None, 5_000, 1_000));
        assert!(!outside_debounce_window(Some(1_000), 1_100, 1_000));
        // The window is inclusive at its end: a full 1000 ms later passes.
        assert!(outside_debounce_window(Some(1_000), 2_000, 1_000));
        // A zero window gates nothing.
        assert!(outside_debounce_window(Some(1_000), 1_001, 0));
    }

    #[test]
    fn bring_up_baselines_a_low_line_and_arms() {
        let gpio = SharedGpio::new([false]);
        let mut m = driver(Box::new(gpio.clone()), Box::new(SharedClock::new(0)), 1_000);
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        assert_eq!(m.hsm.current(), ARMING);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), ARMED);
        assert_eq!(m.data.motions, 0);
        assert_eq!(m.data.suppressed, 0);
        // Exactly one sample so far: the arming baseline.
        assert_eq!(gpio.sampled(), [false]);
    }

    #[test]
    fn bring_up_mid_motion_is_an_accepted_event() {
        // One sample: the arming baseline, already high.
        let gpio = SharedGpio::new([true]);
        let mut m = driver(Box::new(gpio.clone()), Box::new(SharedClock::new(0)), 1_000);
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), MOTION);
        assert_eq!(m.data.motions, 1);
        assert!(posted.len() == 2 && rising(&posted[0]));
        assert_eq!(posted[1].signal, SIG_MEAS_READY);
        gpio.expect_done();
    }

    #[test]
    fn a_rising_edge_becomes_one_occupancy_event_and_the_hold_ends_it() {
        let gpio = SharedGpio::new([false, false, true, true, false]);
        let mut m = driver(Box::new(gpio.clone()), Box::new(SharedClock::new(0)), 1_000);
        let mut posted = Vec::new();
        let mut sink = |event: Event| posted.push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out); // baseline low → ARMED
        m.handle(&Event::new(SIG_TICK), &mut out); // low → stay
        assert_eq!(m.hsm.current(), ARMED);
        m.handle(&Event::new(SIG_TICK), &mut out); // rising edge → MOTION
        assert_eq!(m.hsm.current(), MOTION);
        assert_eq!(m.data.motions, 1);
        assert_eq!(m.data.last_motion_ms, Some(0));
        m.handle(&Event::new(SIG_TICK), &mut out); // held high → stay
        assert_eq!(m.hsm.current(), MOTION);
        assert_eq!(m.data.motions, 1, "a held-high line counts once");
        m.handle(&Event::new(SIG_TICK), &mut out); // falling → ARMED
        assert_eq!(m.hsm.current(), ARMED);
        assert!(posted.len() == 2 && rising(&posted[0]));
        assert_eq!(posted[1].signal, SIG_MEAS_READY);
        gpio.expect_done();
    }

    #[test]
    fn chatter_inside_the_debounce_window_is_suppressed_but_visible() {
        let gpio = SharedGpio::new([false, false, true, false, true, false, true]);
        let clock = SharedClock::new(0);
        let mut m = driver(Box::new(gpio.clone()), Box::new(clock.clone()), 1_000);
        // A shared log: the out-borrow lives to the end of the flow, so
        // mid-flow event counts read through the Rc instead of the Vec.
        let posted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = posted.clone();
        let mut sink = move |event: Event| posted.borrow_mut().push(event);
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out); // baseline: script[0] = low
        clock.advance(100);
        m.handle(&Event::new(SIG_TICK), &mut out); // arming → ARMED (no sample)
        clock.advance(100);
        m.handle(&Event::new(SIG_TICK), &mut out); // t=200: low → stay
        clock.advance(100);
        m.handle(&Event::new(SIG_TICK), &mut out); // t=300: rising edge → accepted
        assert_eq!(m.hsm.current(), MOTION);
        assert_eq!(m.data.motions, 1);
        clock.advance(100);
        m.handle(&Event::new(SIG_TICK), &mut out); // falling at 400 → ARMED
        clock.advance(100);
        m.handle(&Event::new(SIG_TICK), &mut out); // edge at 500: 200 ms < window
        assert_eq!(m.hsm.current(), MOTION);
        assert_eq!(m.data.motions, 1, "inside the window: not motion again");
        assert_eq!(m.data.suppressed, 1, "the dropped edge stays visible");
        assert_eq!(log.borrow().len(), 2, "no second event posted");
        clock.advance(100);
        m.handle(&Event::new(SIG_TICK), &mut out); // falling at 600 → ARMED
        clock.advance(1_100);
        m.handle(&Event::new(SIG_TICK), &mut out); // edge at 1700: 1400 ms past
        assert_eq!(m.hsm.current(), MOTION);
        assert_eq!(m.data.motions, 2, "past the window: accepted");
        assert_eq!(m.data.suppressed, 1);
        gpio.expect_done();
    }

    #[test]
    fn repeated_engages_are_idempotent_while_online() {
        // One sample: the arming baseline (no further samples while idle).
        let gpio = SharedGpio::new([false]);
        let mut m = driver(Box::new(gpio.clone()), Box::new(SharedClock::new(0)), 1_000);
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out);
        assert_eq!(m.hsm.current(), ARMED);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        assert_eq!(m.hsm.current(), ARMED);
        assert_eq!(m.data.motions, 0);
        gpio.expect_done();
    }

    #[test]
    fn the_sample_log_pins_the_tick_to_sample_alignment() {
        let gpio = SharedGpio::new([false, false, true, false, true]);
        let mut m = driver(Box::new(gpio.clone()), Box::new(SharedClock::new(0)), 1_000);
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        m.handle(&Event::new(SIG_TICK), &mut out); // baseline: false
        m.handle(&Event::new(SIG_TICK), &mut out); // armed: false
        m.handle(&Event::new(SIG_TICK), &mut out); // armed: true → MOTION
        m.handle(&Event::new(SIG_TICK), &mut out); // motion: false → ARMED
        m.handle(&Event::new(SIG_TICK), &mut out); // armed: true → MOTION
        assert_eq!(gpio.sampled(), [false, false, true, false, true]);
        gpio.expect_done();
    }
}
