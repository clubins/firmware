//! The cyclic executive: hardware ticks in, fixed phases out.
//!
//! One grid tick is one invocation of the schedule. The caller — the timer
//! ISR's foreground service loop on the device, the test harness on the host —
//! reports how many ticks accumulated and the executive runs every due phase
//! exactly once, in table order, on the grid.
//!
//! [`Executive::advance`] models the foreground loop honestly: when `ticks`
//! exceed one, the later slots of the batch were serviced late (the foreground
//! was busy), and each phase's lateness is recorded in the
//! [`JitterTracker`](self::jitter::JitterTracker) so the bounded-jitter
//! guarantee is *measured*, not assumed.

mod jitter;
mod phase;
mod spsc;
mod wheel;

pub use jitter::JitterTracker;
pub use phase::{DueSlots, MAX_PHASES, PhaseDef, PhaseId, PhaseTable, TableError};
pub use spsc::{Full, SpscRing};
pub use wheel::{DEFAULT_SLOT_CAP, DEFAULT_SLOTS, Fired, TimerId, TimerWheel, WheelError};

/// Grid slots serviced per `advance()` call. A foreground stall longer than
/// this drops grid slots — reported in the batch report, never silently.
pub const MAX_CATCHUP: u32 = 64;

pub const WHEEL_SLOTS: usize = DEFAULT_SLOTS;
pub const WHEEL_SLOT_CAP: usize = DEFAULT_SLOT_CAP;

/// Per-phase work, registered before the executive runs. A plain function
/// pointer: statically known, no allocation, deterministic dispatch.
pub type PhaseHandler = fn(&mut PhaseCtx);

/// What a phase may touch when it runs.
pub struct PhaseCtx<'a> {
    /// Slow cadences (report intervals, heartbeats) scheduled on the wheel.
    pub wheel: &'a mut TimerWheel<WHEEL_SLOTS, WHEEL_SLOT_CAP>,
    /// Absolute grid tick the executive is servicing.
    pub now: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecError {
    /// The phase id is out of range for the handler table.
    PhaseIdOutOfRange,
    /// The phase id is not in the schedule.
    PhaseNotInTable,
    /// A handler is already registered for this phase id.
    DuplicateHandler,
}

/// One phase's activity over a batch of grid slots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhaseRun {
    pub runs: u32,
    /// Worst lateness within the batch, in grid ticks.
    pub max_lateness: u32,
}

/// Aggregate result of a batch of grid slots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchReport {
    /// Indexed by table position (execution order).
    pub per_phase: [PhaseRun; MAX_PHASES],
    pub wheel: Fired,
    /// Grid slots the caller reported but the executive never serviced
    /// (foreground stalled past [`MAX_CATCHUP`]).
    pub dropped_ticks: u64,
    pub ticks_ran: u32,
}

pub struct Executive {
    table: PhaseTable,
    handlers: [Option<PhaseHandler>; MAX_PHASES],
    wheel: TimerWheel<WHEEL_SLOTS, WHEEL_SLOT_CAP>,
    jitter: JitterTracker,
    /// Next grid tick to service.
    tick: u64,
}

impl Executive {
    pub fn new(table: PhaseTable) -> Self {
        Self {
            table,
            handlers: [None; MAX_PHASES],
            wheel: TimerWheel::new(),
            jitter: JitterTracker::default(),
            tick: 0,
        }
    }

    /// Attach the phase's work. Must happen before the schedule runs.
    pub fn register(&mut self, id: PhaseId, handler: PhaseHandler) -> Result<(), ExecError> {
        if id.0 as usize >= MAX_PHASES {
            return Err(ExecError::PhaseIdOutOfRange);
        }
        if !self.table.contains(id) {
            return Err(ExecError::PhaseNotInTable);
        }
        if self.handlers[id.0 as usize].is_some() {
            return Err(ExecError::DuplicateHandler);
        }
        self.handlers[id.0 as usize] = Some(handler);
        Ok(())
    }

    /// The slow-cadence wheel, for scheduling before (or from) phases.
    pub fn wheel_mut(&mut self) -> &mut TimerWheel<WHEEL_SLOTS, WHEEL_SLOT_CAP> {
        &mut self.wheel
    }

    /// Grid tick the executive will service next.
    pub fn next_tick(&self) -> u64 {
        self.tick
    }

    /// Jitter ledger, for asserting the bounded-jitter guarantee.
    pub fn jitter(&self) -> &JitterTracker {
        &self.jitter
    }

    /// Service one grid slot.
    pub fn tick(&mut self) -> BatchReport {
        self.advance(1)
    }

    /// Service `ticks` accumulated grid slots. With `ticks > 1` the foreground
    /// was late: earlier slots of the batch run with recorded lateness, so the
    /// schedule catches up in table order and the jitter ledger stays honest.
    pub fn advance(&mut self, ticks: u32) -> BatchReport {
        let mut report = BatchReport::default();
        let serviceable = ticks.min(MAX_CATCHUP);
        report.dropped_ticks = (ticks - serviceable) as u64;

        for j in 0..serviceable {
            // Slot j of the batch was flagged for service when the foreground
            // noticed the backlog, so it runs (serviceable-1-j) ticks late.
            let lateness = serviceable - 1 - j;
            self.run_slot(lateness, &mut report);
            self.tick = self.tick.wrapping_add(1);
            report.ticks_ran += 1;
            report.wheel.merge(&self.wheel.advance());
        }
        report
    }

    fn run_slot(&mut self, lateness: u32, report: &mut BatchReport) {
        for slot in self.table.due_slots(self.tick) {
            let def = self.table.def(slot);
            if let Some(handler) = self.handlers[def.id.0 as usize] {
                let mut ctx = PhaseCtx {
                    wheel: &mut self.wheel,
                    now: self.tick,
                };
                handler(&mut ctx);
            }
            self.jitter.record(slot, lateness);
            let run = &mut report.per_phase[slot];
            run.runs += 1;
            run.max_lateness = run.max_lateness.max(lateness);
        }
    }
}
