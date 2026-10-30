// The file pins the BH1750 and PIR drivers — it needs both features on
// to name their modules at all.
#![cfg(all(feature = "sensor-pax", feature = "sensor-bh1750"))]
//! Event and debounce flow for the light + occupancy driver pair: the
//! real BH1750 and PIR driver machines, driven through the central
//! dispatcher — the architecture's single execution path — against the
//! same recorded transactions (bus) and level samples (GPIO fixture) the
//! unit tests use.
//!
//! Observation at this layer is intentionally indirect: the dispatcher
//! hands out `Box<dyn Machine>` boundaries, so outcomes are read from the
//! `SharedBus`/`SharedGpio` consumption logs — `expect_done` proves the
//! exact recorded sequence happened, in order, and nothing extra — the
//! dispatcher's own queue accounting, and a spy machine that subscribes
//! to `SIG_PIR_MOTION` to prove the occupancy event and its param reach
//! the rest of the system.

use firmware_v5_hsm::dispatcher::{Dispatcher, Machine};
use firmware_v5_hsm::events::{Event, Param, SIG_ENGAGE, SIG_TICK};
use firmware_v5_hsm::sensors::bh1750::{MeasurementTime, SIG_BH1750_READ};
use firmware_v5_hsm::sensors::bus::{SharedBus, SharedClock, Step};
use firmware_v5_hsm::sensors::gpio::SharedGpio;
use firmware_v5_hsm::sensors::pax::{OCCUPANCY_PER_MOTION, SIG_PIR_MOTION};
use firmware_v5_hsm::sensors::{bh1750, pax};
use std::cell::RefCell;
use std::rc::Rc;

const ADDR: u8 = 0x23;

/// Records the occupancy events the dispatcher delivers to a subscriber.
/// The event log is a shared handle: the dispatcher consumes the machine
/// itself, so the test reads through a clone.
#[derive(Clone, Default)]
struct MotionSpy {
    events: Rc<RefCell<Vec<u32>>>,
}

impl MotionSpy {
    fn events(&self) -> Vec<u32> {
        self.events.borrow().clone()
    }
}

impl Machine for MotionSpy {
    fn id(&self) -> &'static str {
        "motion-spy"
    }

    fn priority(&self) -> u8 {
        // Transports deliver after sensors; the spy rides with them.
        50
    }

    fn subscribes(&self, signal: firmware_v5_hsm::events::Signal) -> bool {
        signal == SIG_PIR_MOTION
    }

    fn handle(&mut self, event: &Event, _ctx: &mut firmware_v5_hsm::fsm::Ctx) {
        if let Param::U32(count) = event.param {
            self.events.borrow_mut().push(count);
        }
    }
}

/// BH1750 bring-up plus one sampling read: power on, MTreg commands,
/// continuous H-res start, then the 2-byte count word (1200 counts).
fn bh1750_full_cycle() -> Vec<Step> {
    let mt = MeasurementTime::DEFAULT.raw();
    let [hi, lo] = [0x40 | (mt >> 5) as u8, 0x60 | (mt & 0x1F) as u8];
    vec![
        Step::Write(ADDR, vec![0x01]), // power on
        Step::Write(ADDR, vec![hi]),   // MTreg high bits
        Step::Write(ADDR, vec![lo]),   // MTreg low bits
        Step::Write(ADDR, vec![0x10]), // continuous H-res
        Step::Read(ADDR, vec![0x04, 0xB0]),
    ]
}

#[test]
fn the_light_driver_completes_a_measurement_through_the_dispatcher() {
    let bus = SharedBus::new(bh1750_full_cycle());
    let clock = SharedClock::new(0);

    let mut d = Dispatcher::new(64);
    d.register(Box::new(bh1750::driver(
        Box::new(bus.clone()),
        Box::new(clock.clone()),
        MeasurementTime::DEFAULT,
    )));
    assert_eq!(d.machine_count(), 1);

    d.start_all();
    // Interleave runs with posts: the first-window deadline is filed
    // when the machine ENTERS WAITING (during the second tick), so the
    // clock must advance between dispatch rounds, not up front.
    assert!(d.post(Event::new(SIG_ENGAGE)));
    d.run_until_idle(128);
    for _ in 0..2 {
        assert!(d.post(Event::new(SIG_TICK)));
        d.run_until_idle(128);
    }
    clock.advance(200); // past the 180 ms first-conversion window
    assert!(d.post(Event::new(SIG_TICK))); // first window elapses
    assert!(d.post(Event::new(SIG_BH1750_READ)));
    assert!(d.post(Event::new(SIG_TICK))); // files the sample
    d.run_until_idle(128);

    // The full scripted register flow happened, in order, and nothing
    // extra; the dispatcher drained to idle with every step accounted.
    bus.expect_done();
    assert!(d.is_empty());
    assert_eq!(d.stats.dropped, 0);
}

#[test]
fn the_pir_edge_flow_posts_occupancy_events_through_the_dispatcher() {
    // Levels per tick: baseline low, low, rising edge (accepted at
    // t=300), falling, rising edge 200 ms later (inside the 1000 ms
    // window → suppressed), falling, then after a 900 ms quiet gap a
    // rising edge 1200 ms after the last accepted one (accepted).
    let gpio = SharedGpio::new([false, false, true, false, true, false, true]);
    let clock = SharedClock::new(0);

    let mut d = Dispatcher::new(64);
    d.register(Box::new(pax::driver(
        Box::new(gpio.clone()),
        Box::new(clock.clone()),
        1_000,
    )));
    let mut spy = MotionSpy::default();
    let spy_log = spy.clone();
    d.register(Box::new(spy));
    assert_eq!(d.machine_count(), 2);

    d.start_all();
    assert!(d.post(Event::new(SIG_ENGAGE)));
    for _ in 0..6 {
        clock.advance(100);
        assert!(d.post(Event::new(SIG_TICK)));
        d.run_until_idle(128);
    }
    // The 900 ms quiet gap pushes the last edge past the debounce window.
    clock.advance(900);
    assert!(d.post(Event::new(SIG_TICK)));
    d.run_until_idle(128);

    // Two accepted motions — the middle edge was suppressed — and the
    // occupancy event (count 1 each) reached the spy machine.
    assert_eq!(
        spy_log.events(),
        vec![OCCUPANCY_PER_MOTION, OCCUPANCY_PER_MOTION]
    );
    gpio.expect_done();
    assert!(d.is_empty());
    assert_eq!(d.stats.dropped, 0);
}

#[test]
fn both_drivers_bring_up_without_interference() {
    // Bring-up only: the BH1750's four configuration writes; the PIR's
    // single baseline sample (then idle-low for the remaining ticks).
    let bus = SharedBus::new(bh1750_full_cycle()[..4].to_vec());
    let gpio = SharedGpio::new([false, false, false]);
    let clock = SharedClock::new(0);

    let mut d = Dispatcher::new(64);
    d.register(Box::new(bh1750::driver(
        Box::new(bus.clone()),
        Box::new(clock.clone()),
        MeasurementTime::DEFAULT,
    )));
    d.register(Box::new(pax::driver(
        Box::new(gpio.clone()),
        Box::new(clock.clone()),
        1_000,
    )));

    d.start_all();
    assert!(d.post(Event::new(SIG_ENGAGE)));
    d.run_until_idle(128);
    for _ in 0..3 {
        clock.advance(200);
        assert!(d.post(Event::new(SIG_TICK)));
        d.run_until_idle(128);
    }

    bus.expect_done();
    assert!(d.is_empty());
    assert_eq!(d.stats.dropped, 0);
}
