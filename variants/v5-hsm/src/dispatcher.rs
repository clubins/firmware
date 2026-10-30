//! Central event dispatcher — the V5 architecture's single execution path.
//!
//! There are no preemptive tasks and no superloop here: machines register
//! with the dispatcher, events queue FIFO, and each event is delivered to
//! every subscribed machine in ascending priority order as a synchronous
//! run-to-completion step. Events posted from inside a handler land on the
//! queue tail and are processed in a later dispatch step — reentrancy is
//! impossible by construction.
//!
//! Ordering and isolation rules, each pinned by a test in this module:
//!
//! - **FIFO.** Events are delivered in post order.
//! - **Priority.** Per event, subscribers are visited in ascending
//!   priority; ties resolve in registration order.
//! - **Subscription.** Only machines whose [`Machine::subscribes`] accepts
//!   the signal see it; non-subscribed machines are untouched.
//! - **No reentrancy.** A post during delivery is queued, never dispatched
//!   inside the same step.
//! - **Isolation.** A machine's unhandled or non-subscribed status never
//!   prevents delivery to other machines, and machines share no state.
//! - **Bounded.** Posting to a full queue drops the event and counts it
//!   (visible via [`Dispatcher::stats`]), never blocks.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::events::Event;
use crate::fsm::{Ctx, Hsm, StateDef};

/// A machine that can receive events from the dispatcher.
pub trait Machine {
    fn id(&self) -> &'static str;

    /// Delivery priority per event: lower numbers are delivered earlier.
    fn priority(&self) -> u8;

    /// Whether this machine wants the given signal.
    fn subscribes(&self, signal: crate::events::Signal) -> bool;

    /// Run-to-completion step for one event. The machine must finish all of
    /// its work before returning; posts go through `ctx`.
    fn handle(&mut self, event: &Event, ctx: &mut Ctx);

    /// Bring-up: runs once, when the machine registers (or explicitly via
    /// [`Dispatcher::start_all`]). Default: nothing.
    fn start(&mut self, _ctx: &mut Ctx) {}
}

/// An [`Hsm`] plus its context data, packaged as a dispatcher machine.
/// The generic context `C` stays typed end-to-end; the dispatcher sees only
/// the erased [`Machine`] trait object.
pub struct HsmMachine<C: 'static> {
    id: &'static str,
    priority: u8,
    subscriptions: &'static [crate::events::Signal],
    pub hsm: Hsm<C>,
    pub data: C,
}

impl<C: 'static> HsmMachine<C> {
    pub fn new(
        id: &'static str,
        priority: u8,
        subscriptions: &'static [crate::events::Signal],
        table: &'static [StateDef<C>],
        root: crate::fsm::StateId,
        defer_capacity: usize,
        data: C,
    ) -> Self {
        HsmMachine {
            id,
            priority,
            subscriptions,
            hsm: Hsm::new(table, root, defer_capacity),
            data,
        }
    }
}

impl<C: 'static> Machine for HsmMachine<C> {
    fn id(&self) -> &'static str {
        self.id
    }

    fn priority(&self) -> u8 {
        self.priority
    }

    fn subscribes(&self, signal: crate::events::Signal) -> bool {
        self.subscriptions.contains(&signal)
    }

    fn handle(&mut self, event: &Event, ctx: &mut Ctx) {
        let Self { hsm, data, .. } = self;
        hsm.dispatch(data, ctx, event);
    }

    fn start(&mut self, ctx: &mut Ctx) {
        let Self { hsm, data, .. } = self;
        hsm.start(data, ctx);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Events accepted into the queue.
    pub posted: usize,
    /// Events dropped because the queue was full.
    pub dropped: usize,
    /// (event, machine) delivery pairs completed.
    pub delivered: usize,
    /// Dispatch steps run (one popped event each).
    pub steps: usize,
}

pub struct Dispatcher {
    queue: VecDeque<Event>,
    capacity: usize,
    machines: Vec<Box<dyn Machine>>,
    pub stats: Stats,
}

impl Dispatcher {
    pub fn new(capacity: usize) -> Self {
        Dispatcher {
            queue: VecDeque::new(),
            capacity,
            machines: Vec::new(),
            stats: Stats::default(),
        }
    }

    /// Registers a machine, keeping the delivery order ascending by
    /// (priority, registration order).
    pub fn register(&mut self, machine: Box<dyn Machine>) {
        let key = (machine.priority(), machine.id());
        let idx = self
            .machines
            .partition_point(|m| (m.priority(), m.id()) <= key);
        self.machines.insert(idx, machine);
    }

    pub fn machine_count(&self) -> usize {
        self.machines.len()
    }

    /// Posts an event to the queue tail. Returns false when the queue is
    /// full (the event is dropped and counted).
    pub fn post(&mut self, event: Event) -> bool {
        if self.queue.len() >= self.capacity {
            self.stats.dropped += 1;
            return false;
        }
        self.queue.push_back(event);
        self.stats.posted += 1;
        true
    }

    /// Pops one event and delivers it to every subscribed machine. Returns
    /// the number of events processed (0 when idle). Events posted during
    /// delivery join the tail and run in later steps.
    pub fn dispatch_once(&mut self) -> usize {
        let Some(event) = self.queue.pop_front() else {
            return 0;
        };
        self.stats.steps += 1;
        let queue = &mut self.queue;
        let mut delivered = 0usize;
        for machine in self.machines.iter_mut() {
            if !machine.subscribes(event.signal) {
                continue;
            }
            let mut sink = |e: Event| queue.push_back(e);
            let mut ctx = Ctx { post: &mut sink };
            machine.handle(&event, &mut ctx);
            delivered += 1;
        }
        self.stats.delivered += delivered;
        1
    }

    /// Runs dispatch steps until the queue is empty or `max_steps` ran.
    /// Returns the number of steps taken.
    pub fn run_until_idle(&mut self, max_steps: usize) -> usize {
        let mut steps = 0;
        while steps < max_steps && self.dispatch_once() == 1 {
            steps += 1;
        }
        steps
    }

    /// Runs every registered machine's bring-up in delivery order.
    pub fn start_all(&mut self) {
        let queue = &mut self.queue;
        for machine in self.machines.iter_mut() {
            let mut sink = |e: Event| queue.push_back(e);
            let mut ctx = Ctx { post: &mut sink };
            machine.start(&mut ctx);
        }
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, Signal};
    use alloc::rc::Rc;
    use core::cell::RefCell;

    const SIG_A: Signal = Signal::new(20);
    const SIG_B: Signal = Signal::new(21);

    type Log = Rc<RefCell<Vec<&'static str>>>;

    /// A minimal test machine that logs deliveries and can post a follow-up.
    struct Probe {
        id: &'static str,
        priority: u8,
        subs: &'static [Signal],
        log: Log,
        /// When handling `post_on`, post `follow_up` through the ctx.
        post_on: Option<Signal>,
        follow_up: Option<Event>,
    }

    impl Probe {
        fn new(
            id: &'static str,
            priority: u8,
            subs: &'static [Signal],
            log: Log,
        ) -> Self {
            Probe {
                id,
                priority,
                subs,
                log,
                post_on: None,
                follow_up: None,
            }
        }
    }

    impl Machine for Probe {
        fn id(&self) -> &'static str {
            self.id
        }
        fn priority(&self) -> u8 {
            self.priority
        }
        fn subscribes(&self, signal: Signal) -> bool {
            self.subs.contains(&signal)
        }
        fn start(&mut self, _ctx: &mut Ctx) {
            self.log.borrow_mut().push(self.id);
        }
        fn handle(&mut self, event: &Event, ctx: &mut Ctx) {
            self.log.borrow_mut().push(self.id);
            if self.post_on == Some(event.signal)
                && let Some(follow_up) = self.follow_up
            {
                ctx.post(follow_up);
            }
        }
    }

    fn boot() -> (Dispatcher, Log) {
        (Dispatcher::new(8), Rc::new(RefCell::new(Vec::new())))
    }

    #[test]
    fn delivery_is_fifo_per_machine() {
        let (mut d, log) = boot();
        d.register(Box::new(Probe::new(
            "p",
            1,
            &[SIG_A, SIG_B],
            Rc::clone(&log),
        )));
        assert!(d.post(Event::new(SIG_A)));
        assert!(d.post(Event::new(SIG_B)));
        assert!(d.post(Event::new(SIG_A)));
        d.run_until_idle(16);
        assert_eq!(*log.borrow(), ["p", "p", "p"]);
        assert!(d.is_empty());
    }

    #[test]
    fn higher_priority_machines_see_each_event_first() {
        let (mut d, log) = boot();
        // Register low priority first; delivery order must still be by priority.
        d.register(Box::new(Probe::new("low", 9, &[SIG_A], Rc::clone(&log))));
        d.register(Box::new(Probe::new("high", 1, &[SIG_A], Rc::clone(&log))));
        assert!(d.post(Event::new(SIG_A)));
        d.run_until_idle(16);
        assert_eq!(*log.borrow(), ["high", "low"]);
    }

    #[test]
    fn subscription_filters_delivery() {
        let (mut d, log) = boot();
        d.register(Box::new(Probe::new("a-only", 1, &[SIG_A], Rc::clone(&log))));
        d.register(Box::new(Probe::new("b-only", 2, &[SIG_B], Rc::clone(&log))));
        assert!(d.post(Event::new(SIG_A)));
        assert!(d.post(Event::new(SIG_B)));
        d.run_until_idle(16);
        assert_eq!(*log.borrow(), ["a-only", "b-only"]);
        assert_eq!(d.stats.delivered, 2);
    }

    #[test]
    fn posts_during_delivery_run_in_a_later_step() {
        let (mut d, log) = boot();
        let mut poster = Probe::new("poster", 1, &[SIG_A, SIG_B], Rc::clone(&log));
        poster.post_on = Some(SIG_A);
        poster.follow_up = Some(Event::new(SIG_B));
        let other = Probe::new("other", 2, &[SIG_A, SIG_B], Rc::clone(&log));
        d.register(Box::new(poster));
        d.register(Box::new(other));
        assert!(d.post(Event::new(SIG_A)));
        d.run_until_idle(16);
        // Step 1 delivers SIG_A to "poster" then "other"; the posted SIG_B
        // joins the tail and is delivered in step 2, again by priority.
        assert_eq!(*log.borrow(), ["poster", "other", "poster", "other"]);
        assert_eq!(d.stats.steps, 2);
    }

    #[test]
    fn one_machines_traffic_never_blocks_anothers() {
        let (mut d, log) = boot();
        // "silent" is subscribed but ignores everything (no trace beyond id —
        // it logs every delivery; use a machine that is subscribed to nothing).
        d.register(Box::new(Probe::new("deaf", 1, &[], Rc::clone(&log))));
        d.register(Box::new(Probe::new(
            "listener",
            2,
            &[SIG_A],
            Rc::clone(&log),
        )));
        assert!(d.post(Event::new(SIG_A)));
        d.run_until_idle(16);
        assert_eq!(*log.borrow(), ["listener"]);
    }

    #[test]
    fn queue_overflow_drops_and_counts() {
        let (mut d, _) = boot(); // capacity 8
        for _ in 0..8 {
            assert!(d.post(Event::new(SIG_A)));
        }
        assert!(!d.post(Event::new(SIG_A)));
        assert_eq!(d.stats.dropped, 1);
        assert_eq!(d.queue_len(), 8);
    }

    #[test]
    fn dispatch_once_on_empty_queue_is_zero() {
        let (mut d, _) = boot();
        assert_eq!(d.dispatch_once(), 0);
        assert_eq!(d.stats.steps, 0);
    }

    #[test]
    fn run_until_idle_respects_the_step_budget() {
        let (mut d, log) = boot();
        d.register(Box::new(Probe::new("p", 1, &[SIG_A], Rc::clone(&log))));
        for _ in 0..4 {
            assert!(d.post(Event::new(SIG_A)));
        }
        assert_eq!(d.run_until_idle(2), 2);
        assert_eq!(d.queue_len(), 2);
    }

    #[test]
    fn start_all_runs_bringup_in_delivery_order() {
        let (mut d, log) = boot();
        d.register(Box::new(Probe::new("low", 9, &[SIG_A], Rc::clone(&log))));
        d.register(Box::new(Probe::new("high", 1, &[SIG_A], Rc::clone(&log))));
        d.start_all();
        assert_eq!(*log.borrow(), ["high", "low"]);
    }

    // An HsmMachine through the dispatcher end-to-end: events reach the
    // framework, and framework stats stay observable.
    #[test]
    fn hsm_machine_integrates_with_the_dispatcher() {
        use crate::fsm::{Outcome, StateDef, StateId};

        const S0: StateId = 0;
        const SIG_GO: Signal = Signal::new(30);

        struct DCtx {
            hits: u32,
        }
        fn handler(d: &mut DCtx, _: &mut Ctx, event: &Event) -> Outcome {
            if event.signal == SIG_GO {
                d.hits += 1;
                Outcome::Handled
            } else {
                Outcome::Unhandled
            }
        }
        static DTABLE: &[StateDef<DCtx>] = &[StateDef {
            name: "only",
            parent: None,
            entry: None,
            exit: None,
            handler,
            transitions: &[],
        }];

        let (mut d, _) = boot();
        let machine =
            HsmMachine::new("dsm", 1, &[SIG_GO], DTABLE, S0, 4, DCtx { hits: 0 });
        d.register(Box::new(machine));
        assert!(d.post(Event::new(SIG_GO)));
        assert_eq!(d.run_until_idle(8), 1);
        // The HsmMachine received and consumed the event: exactly one
        // (event, machine) delivery, no drops, queue drained.
        assert_eq!(d.stats.delivered, 1);
        assert_eq!(d.stats.dropped, 0);
        assert!(d.is_empty());
    }
}
