//! Hierarchical state-machine framework (QP-style).
//!
//! A machine is a static table of [`StateDef`] nodes; nesting is declared
//! via each node's `parent`. Every subsystem owns one [`Hsm`] plus its own
//! context data, all driven by the central dispatcher (`crate::dispatcher`).
//!
//! # Semantics
//!
//! Each rule below is pinned by a test in this module:
//!
//! - **Dispatch walk.** An event enters at the active leaf and climbs the
//!   parent chain until some state consumes it. At each state, the static
//!   transition table is consulted first; if no row matches, the state's
//!   handler decides. [`Outcome::Unhandled`] at the root discards the event
//!   (counted in [`Hsm::stats`]).
//! - **Transitions.** A transition exits from the *active leaf* up to (not
//!   including) the least common ancestor (LCA) of leaf and target —
//!   innermost first — then runs the transition action, then enters from
//!   the LCA down to the target — outermost first. Self-transitions are
//!   external: they exit and re-enter the state (their LCA is the parent).
//! - **No re-entry on ancestor targets.** Transitioning from a nested state
//!   to an enclosing state does not re-run the enclosing state's entry
//!   (QP rule); the framework re-dispatches `INIT`, which re-runs the
//!   composite's initial transition.
//! - **Initial transitions.** After entering any state the framework
//!   dispatches `INIT` to it. A handler (or table row) answering `INIT`
//!   with [`Outcome::Transit`] must target a strict descendant; the child
//!   chain is entered and `INIT` re-dispatches there, nesting until some
//!   state does not transit.
//! - **Deferred events.** [`Outcome::Defer`] moves the current event to the
//!   machine's FIFO defer queue. [`Hsm::recall`] pops the oldest deferred
//!   event and re-dispatches it; if the state hierarchy does not handle it,
//!   the event is dropped, not re-deferred (QP `QHsm_recall`). A defer
//!   arriving at a full queue drops the event and counts it — the machine
//!   keeps running, and the loss is visible in stats, never silent.

use alloc::collections::VecDeque;

use crate::events::{Event, Signal};

pub type StateId = u8;

/// Maximum nesting depth below the root. Tables are validated against this
/// at construction, and it bounds the initial-transition loop.
pub const MAX_HIERARCHY: usize = 8;

/// What a state handler decides for the event it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Consume the event.
    Handled,
    /// Do not consume: propagate to the parent state, if any.
    Unhandled,
    /// Consume the event and park it on the machine's defer queue.
    Defer,
    /// Consume the event and request a transition to `target`.
    Transit(StateId),
}

/// Handle for posting follow-up events from inside handlers, entries, and
/// exits. During dispatch, posted events land on the dispatcher queue tail —
/// they never reenter the current run-to-completion step.
pub struct Ctx<'a> {
    // Crate-visible: the dispatcher constructs a Ctx per delivery step; it
    // is not part of the public API surface of the crate.
    pub(crate) post: &'a mut dyn FnMut(Event),
}

impl Ctx<'_> {
    pub fn post(&mut self, event: Event) {
        (self.post)(event);
    }
}

pub type Entry<C> = fn(&mut C, &mut Ctx);
pub type Exit<C> = fn(&mut C, &mut Ctx);
pub type Action<C> = fn(&mut C, &mut Ctx, &Event);
pub type Handler<C> = fn(&mut C, &mut Ctx, &Event) -> Outcome;

/// A state that does nothing but pass events up the hierarchy. Useful as
/// the handler of pure composite states.
pub fn unhandled<C>(_: &mut C, _: &mut Ctx, _: &Event) -> Outcome {
    Outcome::Unhandled
}

/// One row of a state's static transition table. Rows are consulted before
/// the state's handler; the first row matching the signal wins.
pub struct TransitionRow<C> {
    pub signal: Signal,
    pub target: StateId,
    pub action: Option<Action<C>>,
}

/// A state definition: its position in the hierarchy plus its behavior.
pub struct StateDef<C: 'static> {
    pub name: &'static str,
    pub parent: Option<StateId>,
    pub entry: Option<Entry<C>>,
    pub exit: Option<Exit<C>>,
    pub handler: Handler<C>,
    pub transitions: &'static [TransitionRow<C>],
}

/// Counters exposing what the machine did — a scaffold's way of keeping
/// every loss observable rather than silent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Events parked on the defer queue (successfully).
    pub deferred: usize,
    /// Events lost: defer refused (queue full) or recalled and unhandled.
    pub deferred_dropped: usize,
    /// Deferred events recalled and handled.
    pub recalled: usize,
    /// Events discarded because no state handled them.
    pub unhandled: usize,
    /// Completed transitions (including initial transitions).
    pub transitions: usize,
}

// `C: 'static` because tables are `&'static [StateDef<C>]` — context types
// referenced by a static table must outlive the machine.
pub struct Hsm<C: 'static> {
    table: &'static [StateDef<C>],
    current: StateId,
    deferred: VecDeque<Event>,
    defer_capacity: usize,
    pub stats: Stats,
}

impl<C: 'static> Hsm<C> {
    /// Builds an HSM over a static table. `root` must be a parentless state.
    ///
    /// Panics on malformed tables (out-of-range ids, cycles, multiple roots,
    /// depth beyond [`MAX_HIERARCHY`]) — a static-table bug is a programmer
    /// error that host tests catch before anything ships.
    pub fn new(
        table: &'static [StateDef<C>],
        root: StateId,
        defer_capacity: usize,
    ) -> Self {
        assert!(!table.is_empty(), "state table is empty");
        assert!(
            usize::from(root) < table.len(),
            "root state {root} out of range",
        );
        assert!(
            table[usize::from(root)].parent.is_none(),
            "root state {root} must have no parent",
        );
        for (id, def) in table.iter().enumerate() {
            let id = StateId::try_from(id).expect("state table larger than 255 states");
            if let Some(parent) = def.parent {
                Self::validate_chain(table, id, parent, root);
            }
        }
        Hsm {
            table,
            current: root,
            deferred: VecDeque::new(),
            defer_capacity,
            stats: Stats::default(),
        }
    }

    fn validate_chain(
        table: &'static [StateDef<C>],
        id: StateId,
        parent: StateId,
        root: StateId,
    ) {
        assert!(
            usize::from(parent) < table.len(),
            "state {id}: parent {parent} out of range",
        );
        assert!(parent != id, "state {id} is its own parent");
        let mut cur = parent;
        let mut depth = 1usize;
        loop {
            assert!(
                depth <= MAX_HIERARCHY,
                "state {id}: hierarchy deeper than MAX_HIERARCHY",
            );
            if cur == root {
                return;
            }
            match table[usize::from(cur)].parent {
                Some(next) => {
                    assert!(
                        usize::from(next) < table.len(),
                        "state {id}: ancestor {next} out of range",
                    );
                    cur = next;
                    depth += 1;
                }
                None => panic!("state {id} does not reach the root"),
            }
        }
    }

    /// Enters the initial state: its entry action, then the `INIT` chain.
    pub fn start(&mut self, data: &mut C, out: &mut Ctx) {
        let root = self.current;
        if let Some(entry) = self.table[usize::from(root)].entry {
            entry(data, out);
        }
        self.run_init(data, out);
    }

    /// Run-to-completion dispatch of one event. Returns whether any state
    /// consumed it (handling and deferring both count as consumed).
    pub fn dispatch(&mut self, data: &mut C, out: &mut Ctx, event: &Event) -> bool {
        let mut state = self.current;
        loop {
            let def = &self.table[usize::from(state)];
            let row = def
                .transitions
                .iter()
                .find(|row| row.signal == event.signal)
                .map(|row| (row.target, row.action));
            let (handler, parent) = (def.handler, def.parent);
            if let Some((target, action)) = row {
                self.do_transition(data, out, target, action, event);
                return true;
            }
            match handler(data, out, event) {
                Outcome::Handled => return true,
                Outcome::Defer => {
                    self.push_deferred(event);
                    return true;
                }
                Outcome::Transit(target) => {
                    self.do_transition(data, out, target, None, event);
                    return true;
                }
                Outcome::Unhandled => match parent {
                    Some(p) => state = p,
                    None => {
                        self.stats.unhandled += 1;
                        return false;
                    }
                },
            }
        }
    }

    /// Pop the oldest deferred event and re-dispatch it. Returns whether it
    /// was handled; an unhandled recalled event is dropped (not re-deferred).
    pub fn recall(&mut self, data: &mut C, out: &mut Ctx) -> bool {
        let Some(event) = self.deferred.pop_front() else {
            return false;
        };
        if self.dispatch(data, out, &event) {
            self.stats.recalled += 1;
            true
        } else {
            self.stats.deferred_dropped += 1;
            false
        }
    }

    pub fn current(&self) -> StateId {
        self.current
    }

    pub fn state_name(&self, id: StateId) -> &'static str {
        self.table[usize::from(id)].name
    }

    pub fn is_in(&self, id: StateId) -> bool {
        self.current == id
    }

    pub fn deferred_len(&self) -> usize {
        self.deferred.len()
    }

    fn push_deferred(&mut self, event: &Event) {
        if self.deferred.len() >= self.defer_capacity {
            self.stats.deferred_dropped += 1;
            return;
        }
        self.deferred.push_back(*event);
        self.stats.deferred += 1;
    }

    /// The engine room: exits, transition action, entries, initial chain.
    fn do_transition(
        &mut self,
        data: &mut C,
        out: &mut Ctx,
        target: StateId,
        action: Option<Action<C>>,
        event: &Event,
    ) {
        assert!(
            usize::from(target) < self.table.len(),
            "transition target {target} out of range",
        );
        let source = self.current;
        // External self-transition: LCA is the parent, so the state exits
        // and re-enters itself. (Root self-transitions have no LCA and still
        // exit + re-enter the root.)
        let lca = if target == source {
            self.table[usize::from(source)].parent
        } else {
            Some(self.lca_of(source, target))
        };

        // Exit from the active leaf up to (not including) the LCA.
        let mut s = Some(source);
        while s != lca {
            let cur = s.expect("walk from the leaf stops at the LCA before the root");
            if let Some(exit) = self.table[usize::from(cur)].exit {
                exit(data, out);
            }
            s = self.table[usize::from(cur)].parent;
        }
        if let Some(action) = action {
            action(data, out, event);
        }
        // Enter the chain from the LCA down to the target, outermost first.
        let mut chain = [StateId::MAX; MAX_HIERARCHY];
        let mut n = 0usize;
        let mut s = Some(target);
        while s != lca {
            let cur = s.expect("target chain reaches the LCA before the root");
            assert!(n < MAX_HIERARCHY, "entry chain deeper than MAX_HIERARCHY");
            chain[n] = cur;
            n += 1;
            s = self.table[usize::from(cur)].parent;
        }
        for id in chain[..n].iter().rev() {
            if let Some(entry) = self.table[usize::from(*id)].entry {
                entry(data, out);
            }
        }
        self.current = target;
        self.stats.transitions += 1;
        self.run_init(data, out);
    }

    /// Dispatch `INIT` to the current state and follow initial transitions
    /// down the hierarchy. Initial transitions enter child chains without
    /// exiting the just-entered state.
    fn run_init(&mut self, data: &mut C, out: &mut Ctx) {
        // `=MAX_HIERARCHY` iterations: a full-depth initial chain takes
        // MAX_HIERARCHY transitions, and the leaf at MAX_HIERARCHY still
        // gets its own INIT dispatch in the final iteration (which returns
        // without transiting). Initial transitions strictly descend, so the
        // loop can never outlive the tree depth.
        for _ in 0..=MAX_HIERARCHY {
            let state = self.current;
            let def = &self.table[usize::from(state)];
            let row = def
                .transitions
                .iter()
                .find(|row| row.signal == Signal::INIT)
                .map(|row| row.target);
            let (handler, name) = (def.handler, def.name);
            let target = match row {
                Some(t) => t,
                None => match handler(data, out, &Event::INIT) {
                    Outcome::Transit(t) => t,
                    _ => return,
                },
            };
            assert!(
                self.is_strict_descendant_of(target, state),
                "initial transition from {name} must target a child, not {target}",
            );
            // Enter the child chain without exiting `state` (QP initial rule).
            let mut chain = [StateId::MAX; MAX_HIERARCHY];
            let mut n = 0usize;
            let mut s = target;
            while s != state {
                assert!(n < MAX_HIERARCHY, "entry chain deeper than MAX_HIERARCHY");
                chain[n] = s;
                n += 1;
                s = self.table[usize::from(s)]
                    .parent
                    .expect("validated hierarchy reaches the root");
            }
            for id in chain[..n].iter().rev() {
                if let Some(entry) = self.table[usize::from(*id)].entry {
                    entry(data, out);
                }
            }
            self.current = target;
            self.stats.transitions += 1;
        }
        panic!("initial transitions nest deeper than MAX_HIERARCHY");
    }

    /// Nearest common ancestor of two distinct states in the validated tree.
    fn lca_of(&self, a: StateId, b: StateId) -> StateId {
        let mut marked = [false; 256];
        let mut s = Some(a);
        while let Some(cur) = s {
            marked[usize::from(cur)] = true;
            s = self.table[usize::from(cur)].parent;
        }
        let mut t = b;
        while !marked[usize::from(t)] {
            t = self.table[usize::from(t)]
                .parent
                .expect("validated hierarchy reaches a common root");
        }
        t
    }

    fn is_strict_descendant_of(&self, candidate: StateId, ancestor: StateId) -> bool {
        let mut s = self.table[usize::from(candidate)].parent;
        let mut depth = 0;
        while let Some(cur) = s {
            if cur == ancestor {
                return true;
            }
            depth += 1;
            assert!(depth <= MAX_HIERARCHY, "hierarchy cycle above {candidate}");
            s = self.table[usize::from(cur)].parent;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: StateId = 0; // root
    const S1: StateId = 1; // child of R
    const S11: StateId = 2; // child of S1
    const S12: StateId = 3; // second child of S1
    const S2: StateId = 4; // child of R
    const S21: StateId = 5; // child of S2

    const EV_X: Signal = Signal::new(10); // s11 row → S2 (cross-branch, with action)
    const EV_Y: Signal = Signal::new(11); // s1 row → S12 (sibling switch)
    const EV_Z: Signal = Signal::new(12); // s11 handler → S1 (to ancestor)
    const EV_SELF: Signal = Signal::new(13); // s11 row → S11 (external self)
    const EV_ROOT: Signal = Signal::new(14); // handled at root from any depth
    const EV_DEAD: Signal = Signal::new(15); // nobody handles it
    const EV_DEFER: Signal = Signal::new(16); // deferred, then recalled

    #[derive(Default)]
    struct TCtx {
        trace: Vec<&'static str>,
        handled_at_root: u32,
        /// When true, S21 consumes EV_DEFER; when false, it passes.
        s21_takes_defer: bool,
    }

    fn root_entry(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("enter:root");
    }
    fn root_exit(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("exit:root");
    }
    fn s1_entry(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("enter:s1");
    }
    fn s1_exit(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("exit:s1");
    }
    fn s11_entry(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("enter:s11");
    }
    fn s11_exit(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("exit:s11");
    }
    fn s12_entry(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("enter:s12");
    }
    fn s12_exit(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("exit:s12");
    }
    fn s2_entry(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("enter:s2");
    }
    fn s2_exit(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("exit:s2");
    }
    fn s21_entry(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("enter:s21");
    }
    fn s21_exit(d: &mut TCtx, _: &mut Ctx) {
        d.trace.push("exit:s21");
    }
    fn action_x(d: &mut TCtx, _: &mut Ctx, _: &Event) {
        d.trace.push("action:x");
    }
    fn action_self_row(d: &mut TCtx, _: &mut Ctx, _: &Event) {
        d.trace.push("row:self");
    }

    fn s11_handler(_: &mut TCtx, _: &mut Ctx, event: &Event) -> Outcome {
        match event.signal.raw() {
            0 => Outcome::Handled,       // INIT: leaf, no initial state
            13 => Outcome::Transit(S11), // EV_SELF via handler (row wins first)
            12 => Outcome::Transit(S1),  // EV_Z: to ancestor
            16 => Outcome::Defer,        // EV_DEFER
            _ => Outcome::Unhandled,
        }
    }

    fn s1_handler(_: &mut TCtx, _: &mut Ctx, event: &Event) -> Outcome {
        match event.signal.raw() {
            0 => Outcome::Transit(S11), // INIT: initial state is s11
            _ => Outcome::Unhandled,
        }
    }

    fn s2_handler(_: &mut TCtx, _: &mut Ctx, event: &Event) -> Outcome {
        match event.signal.raw() {
            0 => Outcome::Transit(S21), // INIT
            _ => Outcome::Unhandled,
        }
    }

    fn s21_handler(d: &mut TCtx, _: &mut Ctx, event: &Event) -> Outcome {
        match event.signal.raw() {
            0 => Outcome::Handled, // INIT
            16 => {
                if d.s21_takes_defer {
                    Outcome::Handled
                } else {
                    Outcome::Unhandled
                }
            }
            _ => Outcome::Unhandled,
        }
    }

    fn root_handler(d: &mut TCtx, _: &mut Ctx, event: &Event) -> Outcome {
        match event.signal.raw() {
            0 => Outcome::Transit(S1), // INIT: initial state is s1
            14 => {
                d.handled_at_root += 1;
                Outcome::Handled
            }
            _ => Outcome::Unhandled,
        }
    }

    static TTABLE: &[StateDef<TCtx>] = &[
        StateDef {
            name: "root",
            parent: None,
            entry: Some(root_entry),
            exit: Some(root_exit),
            handler: root_handler,
            transitions: &[],
        },
        StateDef {
            name: "s1",
            parent: Some(R),
            entry: Some(s1_entry),
            exit: Some(s1_exit),
            handler: s1_handler,
            transitions: &[TransitionRow {
                signal: EV_Y,
                target: S12,
                action: None,
            }],
        },
        StateDef {
            name: "s11",
            parent: Some(S1),
            entry: Some(s11_entry),
            exit: Some(s11_exit),
            handler: s11_handler,
            transitions: &[
                TransitionRow {
                    signal: EV_X,
                    target: S2,
                    action: Some(action_x),
                },
                TransitionRow {
                    signal: EV_SELF,
                    target: S11,
                    action: Some(action_self_row),
                },
            ],
        },
        StateDef {
            name: "s12",
            parent: Some(S1),
            entry: Some(s12_entry),
            exit: Some(s12_exit),
            handler: unhandled,
            transitions: &[],
        },
        StateDef {
            name: "s2",
            parent: Some(R),
            entry: Some(s2_entry),
            exit: Some(s2_exit),
            handler: s2_handler,
            transitions: &[],
        },
        StateDef {
            name: "s21",
            parent: Some(S2),
            entry: Some(s21_entry),
            exit: Some(s21_exit),
            handler: s21_handler,
            transitions: &[],
        },
    ];

    fn machine() -> Hsm<TCtx> {
        Hsm::new(TTABLE, R, 4)
    }

    /// Drives `f` with a Ctx that collects posted events. Framework tests
    /// never need the dispatcher for semantics.
    fn with_out(
        f: impl FnOnce(&mut Hsm<TCtx>, &mut TCtx, &mut Ctx),
    ) -> (Hsm<TCtx>, TCtx, Vec<Event>) {
        let mut posted = Vec::new();
        let mut machine = machine();
        let mut data = TCtx::default();
        {
            let mut sink = |e: Event| posted.push(e);
            let mut out = Ctx { post: &mut sink };
            f(&mut machine, &mut data, &mut out);
        }
        (machine, data, posted)
    }

    #[test]
    fn start_runs_entry_chain_in_nesting_order() {
        let (_, data, _) = with_out(|m, d, out| m.start(d, out));
        assert_eq!(data.trace, ["enter:root", "enter:s1", "enter:s11"]);
    }

    #[test]
    fn table_transition_exits_to_lca_then_enters_with_action() {
        let (m, data, _) = with_out(|m, d, out| {
            m.start(d, out);
            d.trace.clear();
            let ev = Event::new(EV_X);
            assert!(m.dispatch(d, out, &ev));
        });
        // Leaf s11 exits, s1 exits (LCA is the root), the action runs, then
        // s2 and its initial child s21 enter.
        assert_eq!(
            data.trace,
            ["exit:s11", "exit:s1", "action:x", "enter:s2", "enter:s21"]
        );
        assert!(m.is_in(S21));
        // Two hops at start (root→s1, s1→s11) plus the dispatch and the
        // initial hop it triggers inside s2 — Stats counts initial
        // transitions too (documented on the field).
        assert_eq!(m.stats.transitions, 4);
    }

    #[test]
    fn sibling_switch_keeps_the_composite_active() {
        let (m, data, _) = with_out(|m, d, out| {
            m.start(d, out);
            d.trace.clear();
            let ev = Event::new(EV_Y); // row on s1: s11 → s12
            assert!(m.dispatch(d, out, &ev));
        });
        // s11 unhandled → s1's table row: exits s11 only (LCA is s1); s1
        // stays active; s12 enters.
        assert_eq!(data.trace, ["exit:s11", "enter:s12"]);
        assert!(m.is_in(S12));
        assert!(!data.trace.contains(&"exit:s1"));
    }

    #[test]
    fn transition_to_ancestor_reinits_instead_of_reentering() {
        let (m, data, _) = with_out(|m, d, out| {
            m.start(d, out);
            d.trace.clear();
            let ev = Event::new(EV_Z); // s11 handler: → s1
            assert!(m.dispatch(d, out, &ev));
        });
        // QP rule: s1 is NOT re-entered; its INIT re-dispatch re-runs the
        // initial transition into s11.
        assert_eq!(data.trace, ["exit:s11", "enter:s11"]);
        assert!(!data.trace.contains(&"enter:s1"));
        assert!(m.is_in(S11));
    }

    #[test]
    fn self_transition_is_external() {
        let (m, data, _) = with_out(|m, d, out| {
            m.start(d, out);
            d.trace.clear();
            let ev = Event::new(EV_SELF); // row on s11: → s11
            assert!(m.dispatch(d, out, &ev));
        });
        assert_eq!(data.trace, ["exit:s11", "row:self", "enter:s11"]);
        assert!(m.is_in(S11));
    }

    #[test]
    fn table_rows_win_over_handlers() {
        // s11 handles EV_SELF in the handler too (same target), but the row
        // must win: its action runs.
        let (_, data, _) = with_out(|m, d, out| {
            m.start(d, out);
            let ev = Event::new(EV_SELF);
            assert!(m.dispatch(d, out, &ev));
        });
        assert!(data.trace.contains(&"row:self"));
    }

    #[test]
    fn unhandled_events_propagate_to_the_root_handler() {
        let (m, data, _) = with_out(|m, d, out| {
            m.start(d, out);
            let ev = Event::new(EV_ROOT);
            assert!(m.dispatch(d, out, &ev));
        });
        assert_eq!(data.handled_at_root, 1);
        assert_eq!(m.stats.unhandled, 0);
        assert!(m.is_in(S11), "handling at the root does not move the leaf");
    }

    #[test]
    fn fully_unhandled_event_is_counted_and_dropped() {
        let (m, _, _) = with_out(|m, d, out| {
            m.start(d, out);
            let ev = Event::new(EV_DEAD);
            assert!(!m.dispatch(d, out, &ev));
        });
        assert_eq!(m.stats.unhandled, 1);
    }

    #[test]
    fn defer_parks_the_event_and_recall_redelivers_to_the_new_state() {
        let (mut m, mut data, _) = with_out(|m, d, out| {
            m.start(d, out); // lands in s11
            let ev = Event::new(EV_DEFER);
            assert!(m.dispatch(d, out, &ev)); // s11 defers it
            assert_eq!(m.deferred_len(), 1);
            assert_eq!(m.stats.deferred, 1);
            // Cross into s21 (via s11's EV_X row) and recall there.
            let ev_x = Event::new(EV_X);
            assert!(m.dispatch(d, out, &ev_x));
            assert!(m.is_in(S21));
        });
        data.s21_takes_defer = true;
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        assert!(m.recall(&mut data, &mut out), "recalled event must land");
        assert_eq!(m.stats.recalled, 1);
        assert_eq!(m.deferred_len(), 0);
    }

    #[test]
    fn recall_drops_an_event_the_state_still_wont_handle() {
        let (mut m, mut data, _) = with_out(|m, d, out| {
            m.start(d, out);
            let ev = Event::new(EV_DEFER);
            assert!(m.dispatch(d, out, &ev));
            let ev_x = Event::new(EV_X);
            assert!(m.dispatch(d, out, &ev_x)); // → s21
            assert!(m.is_in(S21));
        });
        // s21_takes_defer stays false: recall pops the event, it goes
        // unhandled, and it is dropped rather than re-deferred.
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        assert!(!m.recall(&mut data, &mut out));
        assert_eq!(m.deferred_len(), 0);
        assert_eq!(m.stats.deferred_dropped, 1);
        assert_eq!(m.stats.recalled, 0);
    }

    #[test]
    fn defer_at_capacity_drops_and_counts() {
        let mut m = Hsm::new(TTABLE, R, 2);
        let mut data = TCtx::default();
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut data, &mut out);
        for _ in 0..2 {
            assert!(m.dispatch(&mut data, &mut out, &Event::new(EV_DEFER)));
            // Each defer is parked; s11 defers again on re-dispatch, so
            // re-enter s11 between defers via the self row.
            assert!(m.dispatch(&mut data, &mut out, &Event::new(EV_SELF)));
        }
        assert_eq!(m.deferred_len(), 2);
        // Third defer: queue full → dropped and counted, machine keeps going.
        let before = m.stats.deferred_dropped;
        assert!(m.dispatch(&mut data, &mut out, &Event::new(EV_DEFER)));
        assert_eq!(m.deferred_len(), 2);
        assert_eq!(m.stats.deferred_dropped, before + 1);
    }

    #[test]
    fn malformed_table_rejects_cycles() {
        static CYCLIC: &[StateDef<TCtx>] = &[
            StateDef {
                name: "a",
                parent: Some(1),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[],
            },
            StateDef {
                name: "b",
                parent: Some(0),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[],
            },
        ];
        let result = std::panic::catch_unwind(|| Hsm::<TCtx>::new(CYCLIC, 0, 4));
        assert!(result.is_err(), "cyclic hierarchy must be rejected");
    }

    #[test]
    fn initial_transition_to_nondescendant_is_rejected() {
        static BAD_INIT: &[StateDef<TCtx>] = &[
            StateDef {
                name: "root",
                parent: None,
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: 1,
                    action: None,
                }],
            },
            StateDef {
                name: "orphan-ish",
                parent: Some(0),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: 0, // INIT targeting the root itself: forbidden
                    action: None,
                }],
            },
        ];
        let result = std::panic::catch_unwind(|| {
            let mut m = Hsm::<TCtx>::new(BAD_INIT, 0, 4);
            let mut data = TCtx::default();
            let mut sink = |_: Event| {};
            let mut out = Ctx { post: &mut sink };
            m.start(&mut data, &mut out);
        });
        assert!(result.is_err(), "INIT to a non-child must be rejected");
    }

    #[test]
    fn initial_chain_can_reach_the_full_hierarchy_depth() {
        // A legal table may use every level of MAX_HIERARCHY below the root
        // (validate_chain admits depth-8 states). Its initial chain takes
        // MAX_HIERARCHY transitions — root→d1 … d7→d8 — and the depth-8
        // leaf still gets its own INIT dispatch. start() must complete
        // instead of tripping the nesting-depth panic.
        const D1: StateId = 1;
        const D2: StateId = 2;
        const D3: StateId = 3;
        const D4: StateId = 4;
        const D5: StateId = 5;
        const D6: StateId = 6;
        const D7: StateId = 7;
        const D8: StateId = 8;
        static DEEP: &[StateDef<TCtx>] = &[
            StateDef {
                name: "deep-root",
                parent: None,
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: D1,
                    action: None,
                }],
            },
            StateDef {
                name: "d1",
                parent: Some(0),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: D2,
                    action: None,
                }],
            },
            StateDef {
                name: "d2",
                parent: Some(D1),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: D3,
                    action: None,
                }],
            },
            StateDef {
                name: "d3",
                parent: Some(D2),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: D4,
                    action: None,
                }],
            },
            StateDef {
                name: "d4",
                parent: Some(D3),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: D5,
                    action: None,
                }],
            },
            StateDef {
                name: "d5",
                parent: Some(D4),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: D6,
                    action: None,
                }],
            },
            StateDef {
                name: "d6",
                parent: Some(D5),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: D7,
                    action: None,
                }],
            },
            StateDef {
                name: "d7",
                parent: Some(D6),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[TransitionRow {
                    signal: Signal::INIT,
                    target: D8,
                    action: None,
                }],
            },
            StateDef {
                name: "d8",
                parent: Some(D7),
                entry: None,
                exit: None,
                handler: unhandled,
                transitions: &[],
            },
        ];
        let mut m = Hsm::new(DEEP, 0, 4);
        let mut data = TCtx::default();
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut data, &mut out);
        assert!(m.is_in(D8));
        assert_eq!(m.stats.transitions, 8);
    }
}
