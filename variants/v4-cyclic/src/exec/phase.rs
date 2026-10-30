//! The fixed phase table — in a time-triggered executive, the schedule IS the
//! program.
//!
//! Table order is execution order: within one grid tick, phases run in the
//! order they were added (sensor → processing → comms → ops). A phase runs
//! exactly once per tick where `tick % period == offset`, never twice, and
//! never ahead of a phase earlier in the table.

use core::num::NonZeroU32;

/// Maximum phases in one schedule. Generous for a fixed executive.
pub const MAX_PHASES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhaseId(pub u8);

impl PhaseId {
    pub const SENSOR: Self = Self(0);
    pub const PROCESSING: Self = Self(1);
    pub const COMMS: Self = Self(2);
    pub const OPS: Self = Self(3);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhaseDef {
    pub id: PhaseId,
    /// Run every `period` ticks.
    pub period: NonZeroU32,
    /// Runs on ticks t where `t % period == offset`.
    pub offset: u32,
}

impl PhaseDef {
    /// Every tick. The common case for sensor and processing phases.
    pub fn every_tick(id: PhaseId) -> Self {
        Self {
            id,
            period: NonZeroU32::MIN,
            offset: 0,
        }
    }

    /// Every `period` ticks, starting at tick `offset` (must be < period).
    pub fn every(id: PhaseId, period: NonZeroU32, offset: u32) -> Self {
        Self { id, period, offset }
    }

    pub fn is_due(&self, tick: u64) -> bool {
        tick % self.period.get() as u64 == self.offset as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    /// More than [`MAX_PHASES`] phases.
    Full,
    /// The phase id is already in the table.
    DuplicateId,
    /// `offset >= period` — the phase could never run.
    OffsetBeyondPeriod,
}

pub struct PhaseTable {
    defs: [PhaseDef; MAX_PHASES],
    len: usize,
}

impl PhaseTable {
    pub const fn new() -> Self {
        Self {
            defs: [PhaseDef::PLACEHOLDER; MAX_PHASES],
            len: 0,
        }
    }

    /// Append a phase. Insertion order is execution order.
    pub fn push(&mut self, def: PhaseDef) -> Result<(), TableError> {
        if self.len == MAX_PHASES {
            return Err(TableError::Full);
        }
        if self.defs[..self.len].iter().any(|d| d.id == def.id) {
            return Err(TableError::DuplicateId);
        }
        if def.offset >= def.period.get() {
            return Err(TableError::OffsetBeyondPeriod);
        }
        self.defs[self.len] = def;
        self.len += 1;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The phase at table position `slot` (execution order).
    pub fn def(&self, slot: usize) -> PhaseDef {
        self.defs[slot]
    }

    /// Is this phase id scheduled?
    pub fn contains(&self, id: PhaseId) -> bool {
        self.defs[..self.len].iter().any(|d| d.id == id)
    }

    /// Phases due at `tick`, in table order — the executive's dispatch list.
    pub fn due_slots(&self, tick: u64) -> DueSlots<'_> {
        DueSlots {
            table: self,
            tick,
            next: 0,
        }
    }
}

impl Default for PhaseTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Iterator over table slots due at a fixed tick, in table order.
pub struct DueSlots<'a> {
    table: &'a PhaseTable,
    tick: u64,
    next: usize,
}

impl Iterator for DueSlots<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        while self.next < self.table.len {
            let slot = self.next;
            self.next += 1;
            if self.table.defs[slot].is_due(self.tick) {
                return Some(slot);
            }
        }
        None
    }
}

impl PhaseDef {
    /// Fills unused table capacity; never scheduled.
    const PLACEHOLDER: Self = Self {
        id: PhaseId(0),
        period: NonZeroU32::MIN,
        offset: 0,
    };
}
