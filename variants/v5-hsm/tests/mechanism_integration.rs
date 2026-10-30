//! Mechanism integration tests — the V5 architecture story end to end.
//!
//! The unit tests inside `fsm`, `dispatcher`, and `system` pin each piece in
//! isolation (hand-driven, or with trivial probe machines). These tests run
//! *composed* machines — the real system machine plus HSM-backed subsystem
//! machines — through the real `Dispatcher`, pinning the cross-machine
//! guarantees the architecture claim rests on: subsystem events flow over
//! one central dispatcher, the system machine sees each event first,
//! deferred events re-deliver with their parameters intact, and
//! run-to-completion stays bounded when a machine keeps posting.

use std::cell::RefCell;
use std::rc::Rc;

use firmware_v5_hsm::dispatcher::{Dispatcher, HsmMachine, Machine};
use firmware_v5_hsm::events::{
    Event, Param, SIG_COMPLETE, SIG_ENGAGE, SIG_FAULT, SIG_PUBLISH_FAIL, SIG_PUBLISH_OK,
    SIG_STATUS_HEARTBEAT, SIG_WIFI_UP, Signal,
};
use firmware_v5_hsm::fsm::{Ctx, Hsm, Outcome, StateDef, StateId};
use firmware_v5_hsm::stubkit::{STUB_IDLE, STUB_TABLE, StubCtx};
use firmware_v5_hsm::system::{SYS_FAULT, SYS_PUBLISHING, SYS_SAMPLING, SystemMachine};

// Test-local signals (the module-level ranges in events.rs are for
// production subsystems; tests allocate their own ids, as the framework
// tests in `dispatcher` do).
const SIG_A: Signal = Signal::new(20);
const SIG_P_MEAS: Signal = Signal::new(40);
const SIG_P_RECALL: Signal = Signal::new(41);

/// The real `SystemMachine` wrapped so its internal state is observable
/// after every dispatcher delivery: (current state, deferred queue length,
/// recalled count). The dispatcher owns `Box<dyn Machine>` — a wrapper is
/// the only way to read back machine-internal state through a live queue.
struct Spy {
    system: SystemMachine,
    records: Rc<RefCell<Vec<(StateId, usize, usize)>>>,
    log: Rc<RefCell<Vec<&'static str>>>,
}

impl Spy {
    fn new(
        records: Rc<RefCell<Vec<(StateId, usize, usize)>>>,
        log: Rc<RefCell<Vec<&'static str>>>,
    ) -> Self {
        Spy {
            system: SystemMachine::new(),
            records,
            log,
        }
    }
}

impl Machine for Spy {
    fn id(&self) -> &'static str {
        self.system.id()
    }
    fn priority(&self) -> u8 {
        self.system.priority()
    }
    fn subscribes(&self, signal: Signal) -> bool {
        self.system.subscribes(signal)
    }
    fn start(&mut self, ctx: &mut Ctx) {
        self.system.start(ctx)
    }
    fn handle(&mut self, event: &Event, ctx: &mut Ctx) {
        self.system.handle(event, ctx);
        self.log.borrow_mut().push("system");
        self.records.borrow_mut().push((
            self.system.hsm.current(),
            self.system.hsm.deferred_len(),
            self.system.hsm.stats.recalled,
        ));
    }
}

/// The composed measurement cycle over one dispatcher: a sensor stub's
/// completion posts `SIG_MEAS_READY` (queued — no reentrancy), the system
/// machine publishes, the failure parks offline, the reconnect recalls it.
#[test]
fn measurement_cycle_across_machines_over_one_dispatcher() {
    let mut d = Dispatcher::new(64);
    let records = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::new(RefCell::new(Vec::new()));
    d.register(Box::new(Spy::new(Rc::clone(&records), Rc::clone(&log))));
    d.register(Box::new(HsmMachine::new(
        "sensor-stub",
        1,
        &[SIG_ENGAGE, SIG_COMPLETE],
        STUB_TABLE,
        STUB_IDLE,
        2,
        StubCtx::default(),
    )));

    // Bring-up: the system machine posts SIG_BOOT_DONE from start; one step
    // delivers it and the machine flows BOOT → RUNNING → SAMPLING.
    d.start_all();
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(records.borrow().last(), Some(&(SYS_SAMPLING, 0, 0)));

    // Engage, then complete the sensor: the stub posts SIG_MEAS_READY from
    // inside its handler. It must still be in the queue when the step ends.
    assert!(d.post(Event::new(SIG_ENGAGE)));
    assert_eq!(d.dispatch_once(), 1);
    assert!(d.post(Event::new(SIG_COMPLETE)));
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(
        d.queue_len(),
        1,
        "a follow-up posted mid-handler waits its turn — no reentrancy"
    );
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(records.borrow().last(), Some(&(SYS_PUBLISHING, 0, 0)));

    // The publish fails while offline: the system machine parks the event
    // on its defer queue and stays in PUBLISHING.
    assert!(d.post(Event::new(SIG_PUBLISH_FAIL)));
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(records.borrow().last(), Some(&(SYS_PUBLISHING, 1, 0)));

    // The link comes back: the same WIFI_UP step flips online and recalls —
    // the deferred failure is re-delivered inside the machine, the defer
    // queue drains, and the recalled count ticks.
    assert!(d.post(Event::new(SIG_WIFI_UP)));
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(records.borrow().last(), Some(&(SYS_PUBLISHING, 0, 1)));

    // The retried publish succeeds: back to SAMPLING. Queue fully drained.
    assert!(d.post(Event::new(SIG_PUBLISH_OK)));
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(records.borrow().last(), Some(&(SYS_SAMPLING, 0, 1)));
    assert!(d.is_empty());
    assert_eq!(d.stats.dropped, 0);
}

/// A subsystem machine that latches a fault when it sees a heartbeat. Its
/// own log entry records whether the system machine ran before it — the
/// dispatcher's contract says priority 0 (system) goes first per event.
#[test]
fn the_system_machine_runs_before_subsystem_machines() {
    struct FaultPoster {
        log: Rc<RefCell<Vec<&'static str>>>,
    }
    impl Machine for FaultPoster {
        fn id(&self) -> &'static str {
            "fault-poster"
        }
        fn priority(&self) -> u8 {
            1
        }
        fn subscribes(&self, signal: Signal) -> bool {
            signal == SIG_STATUS_HEARTBEAT
        }
        fn handle(&mut self, _: &Event, ctx: &mut Ctx) {
            let ran_first = self.log.borrow().last() == Some(&"system");
            self.log.borrow_mut().push(if ran_first {
                "poster-after-system"
            } else {
                "poster-first"
            });
            ctx.post(Event::new(SIG_FAULT));
        }
    }

    let mut d = Dispatcher::new(64);
    let records = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::new(RefCell::new(Vec::new()));
    d.register(Box::new(Spy::new(Rc::clone(&records), Rc::clone(&log))));
    d.register(Box::new(FaultPoster {
        log: Rc::clone(&log),
    }));

    d.start_all();
    assert_eq!(d.dispatch_once(), 1); // SIG_BOOT_DONE → system only
    log.borrow_mut().clear();

    // Heartbeat step: system (priority 0) then the poster (priority 1);
    // the fault the poster raises is a follow-up event, not a same-step
    // steal. Fault step: the system machine latches from SAMPLING.
    assert!(d.post(Event::new(SIG_STATUS_HEARTBEAT)));
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(*log.borrow(), ["system", "poster-after-system"]);
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(*log.borrow(), ["system", "poster-after-system", "system"]);
    assert_eq!(records.borrow().last(), Some(&(SYS_FAULT, 0, 0)));
    assert_eq!(d.stats.delivered, 4); // boot 1 + heartbeat 2 + fault 1
}

/// A machine that parks an event with a parameter, then recalls it through
/// the dispatcher: the parameter must arrive intact both times.
#[test]
fn event_params_survive_the_queue_and_defer_recall() {
    #[derive(Default)]
    struct PData {
        seen: Vec<Param>,
    }
    fn p_handler(d: &mut PData, _: &mut Ctx, event: &Event) -> Outcome {
        match event.signal.raw() {
            40 => {
                d.seen.push(event.param);
                if d.seen.len() < 2 {
                    Outcome::Defer
                } else {
                    Outcome::Handled
                }
            }
            41 => Outcome::Handled,
            _ => Outcome::Unhandled,
        }
    }
    static PTABLE: &[StateDef<PData>] = &[StateDef {
        name: "holding",
        parent: None,
        entry: None,
        exit: None,
        handler: p_handler,
        transitions: &[],
    }];

    // Like the system machine, the recall driver is a hand-written Machine
    // whose handle recalls the defer queue on the reconnect-style signal.
    struct ParamMachine {
        hsm: Hsm<PData>,
        data: PData,
        seen: Rc<RefCell<Vec<Param>>>,
    }
    impl Machine for ParamMachine {
        fn id(&self) -> &'static str {
            "param"
        }
        fn priority(&self) -> u8 {
            1
        }
        fn subscribes(&self, signal: Signal) -> bool {
            signal == SIG_P_MEAS || signal == SIG_P_RECALL
        }
        fn start(&mut self, ctx: &mut Ctx) {
            self.hsm.start(&mut self.data, ctx)
        }
        fn handle(&mut self, event: &Event, ctx: &mut Ctx) {
            if event.signal == SIG_P_RECALL {
                self.hsm.recall(&mut self.data, ctx);
            } else {
                self.hsm.dispatch(&mut self.data, ctx, event);
            }
            *self.seen.borrow_mut() = self.data.seen.clone();
        }
    }

    let mut d = Dispatcher::new(64);
    let seen = Rc::new(RefCell::new(Vec::new()));
    d.register(Box::new(ParamMachine {
        hsm: Hsm::new(PTABLE, 0, 4),
        data: PData::default(),
        seen: Rc::clone(&seen),
    }));

    // Leg 1 — through the dispatcher queue: the param rides Event::with_param.
    assert!(d.post(Event::with_param(SIG_P_MEAS, Param::F32(4.5))));
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(*seen.borrow(), [Param::F32(4.5)]);

    // Leg 2 — through defer + recall: the machine re-receives the same
    // parametrized event from its own defer queue.
    assert!(d.post(Event::new(SIG_P_RECALL)));
    assert_eq!(d.dispatch_once(), 1);
    assert_eq!(
        *seen.borrow(),
        [Param::F32(4.5), Param::F32(4.5)],
        "the recalled event keeps its parameter"
    );
    assert!(d.is_empty());
}

/// A machine that answers every event with one follow-up post: the step
/// budget must bound run-to-completion — never spin, never starve.
#[test]
fn a_self_posting_machine_stays_bounded_by_the_step_budget() {
    struct Echo;
    impl Machine for Echo {
        fn id(&self) -> &'static str {
            "echo"
        }
        fn priority(&self) -> u8 {
            1
        }
        fn subscribes(&self, signal: Signal) -> bool {
            signal == SIG_A
        }
        fn handle(&mut self, _: &Event, ctx: &mut Ctx) {
            ctx.post(Event::new(SIG_A));
        }
    }

    let mut d = Dispatcher::new(16);
    d.register(Box::new(Echo));
    assert!(d.post(Event::new(SIG_A)));
    assert_eq!(
        d.run_until_idle(64),
        64,
        "the budget, not the churn, stops the loop"
    );
    assert_eq!(
        d.queue_len(),
        1,
        "one follow-up is still queued, none dropped"
    );
    assert_eq!(d.stats.dropped, 0);
}
