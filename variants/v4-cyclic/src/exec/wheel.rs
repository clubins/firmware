//! Software timer wheel for cadences slower than the tick grid.
//!
//! The wheel has `SLOTS` buckets; an entry lives in slot `expiry % SLOTS`.
//! Each grid tick the executive advances one slot and fires the entries there
//! whose absolute expiry has come. Entries with a farther expiry stay put and
//! fire when their slot comes around again — no round counters to keep in
//! sync. Periodic entries re-arm `period` ticks after firing.

use core::num::NonZeroU32;

pub const DEFAULT_SLOTS: usize = 64;
pub const DEFAULT_SLOT_CAP: usize = 8;
/// Cap on timers reported fired per tick; overflow is flagged, never hidden.
pub const MAX_FIRED_PER_TICK: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimerId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelError {
    /// The wheel's id space is exhausted (u16 ids).
    IdsExhausted,
    /// The target slot is full — cadences too dense for this wheel geometry.
    SlotFull,
    /// The requested timer id is not scheduled.
    NoSuchTimer,
    /// The expiry falls beyond the wheel's grid time range.
    GridTimeOverflow,
}

/// Fired timer ids for one `advance()` call, as a fixed-capacity buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fired {
    pub ids: [Option<TimerId>; MAX_FIRED_PER_TICK],
    pub len: usize,
    /// More timers were due than fit in `ids`; the report is truncated.
    pub truncated: bool,
    /// A periodic re-arm failed ([`WheelError::SlotFull`] or `RearmOverflow`).
    pub rearm_failed: bool,
}

impl Fired {
    fn push(&mut self, id: TimerId) {
        if self.len < MAX_FIRED_PER_TICK {
            self.ids[self.len] = Some(id);
            self.len += 1;
        } else {
            self.truncated = true;
        }
    }

    pub(crate) fn merge(&mut self, other: &Fired) {
        for id in other.ids[..other.len].iter().flatten() {
            self.push(*id);
        }
        self.truncated |= other.truncated;
        self.rearm_failed |= other.rearm_failed;
    }
}

#[derive(Clone, Copy)]
struct Entry {
    id: TimerId,
    /// Absolute grid tick at which this entry fires.
    expiry: u64,
    /// Some(period): re-arm every `period` ticks after firing.
    period: Option<NonZeroU32>,
}

pub struct TimerWheel<const SLOTS: usize, const SLOT_CAP: usize> {
    slots: [[Option<Entry>; SLOT_CAP]; SLOTS],
    lens: [usize; SLOTS],
    /// Absolute grid tick the wheel has advanced to.
    now: u64,
    next_id: u16,
}

impl<const SLOTS: usize, const SLOT_CAP: usize> TimerWheel<SLOTS, SLOT_CAP> {
    const MASK: u64 = SLOTS as u64 - 1;

    pub const fn new() -> Self {
        const {
            assert!(
                SLOTS.is_power_of_two(),
                "wheel slot count must be a power of two"
            );
        }
        Self {
            slots: [[None; SLOT_CAP]; SLOTS],
            lens: [0; SLOTS],
            now: 0,
            next_id: 0,
        }
    }

    /// Schedule a timer firing `delay_ticks` from now (must be ≥ 1: the
    /// wheel only observes slots it advances onto); periodic if `period`
    /// is set. The id is assigned by the wheel.
    pub fn schedule(
        &mut self,
        delay_ticks: u64,
        period: Option<NonZeroU32>,
    ) -> Result<TimerId, WheelError> {
        debug_assert!(
            delay_ticks >= 1,
            "timers fire on a future tick; delay must be >= 1"
        );
        let id = TimerId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(WheelError::IdsExhausted)?;
        let expiry = self
            .now
            .checked_add(delay_ticks)
            .ok_or(WheelError::GridTimeOverflow)?;
        self.insert(id, expiry, period)?;
        Ok(id)
    }

    /// Cancel a scheduled timer. Errors if it already fired or never existed.
    pub fn cancel(&mut self, id: TimerId) -> Result<(), WheelError> {
        for slot in 0..SLOTS {
            let entries = &mut self.slots[slot];
            if let Some(pos) = entries[..self.lens[slot]]
                .iter()
                .position(|e| e.as_ref().is_some_and(|entry| entry.id == id))
            {
                entries[pos] = None;
                self.compact(slot);
                return Ok(());
            }
        }
        Err(WheelError::NoSuchTimer)
    }

    /// Advance one grid tick and fire due timers. Periodic entries re-arm
    /// after firing; a failed re-arm is reported, never swallowed.
    pub fn advance(&mut self) -> Fired {
        self.now = self.now.wrapping_add(1);
        let slot = (self.now & Self::MASK) as usize;
        let mut fired = Fired::default();

        // Re-arms are buffered and inserted after the drain: inserting into
        // this slot mid-drain would collide with the compaction pass below.
        let mut rearm = [None; SLOT_CAP];
        let mut rearm_len = 0;

        let mut kept = 0;
        for i in 0..self.lens[slot] {
            let entry = self.slots[slot][i]
                .take()
                .expect("slot entries are compacted");
            if entry.expiry > self.now {
                self.slots[slot][kept] = Some(entry);
                kept += 1;
                continue;
            }
            fired.push(entry.id);
            if let Some(period) = entry.period {
                match entry.expiry.checked_add(period.get() as u64) {
                    Some(next_expiry) if rearm_len < SLOT_CAP => {
                        rearm[rearm_len] = Some((entry.id, next_expiry, period));
                        rearm_len += 1;
                    }
                    _ => fired.rearm_failed = true,
                }
            }
        }
        self.lens[slot] = kept;
        for entry in &rearm[..rearm_len] {
            let (id, expiry, period) = entry.expect("buffered in order");
            if self.insert(id, expiry, Some(period)).is_err() {
                fired.rearm_failed = true;
            }
        }
        fired
    }

    /// Absolute grid tick the wheel has advanced to.
    pub fn now(&self) -> u64 {
        self.now
    }

    fn insert(
        &mut self,
        id: TimerId,
        expiry: u64,
        period: Option<NonZeroU32>,
    ) -> Result<(), WheelError> {
        let slot = (expiry & Self::MASK) as usize;
        if self.lens[slot] == SLOT_CAP {
            return Err(WheelError::SlotFull);
        }
        self.slots[slot][self.lens[slot]] = Some(Entry { id, expiry, period });
        self.lens[slot] += 1;
        Ok(())
    }

    fn compact(&mut self, slot: usize) {
        let mut kept = 0;
        for i in 0..self.lens[slot] {
            if let Some(entry) = self.slots[slot][i].take() {
                self.slots[slot][kept] = Some(entry);
                kept += 1;
            }
        }
        self.lens[slot] = kept;
    }
}

impl<const SLOTS: usize, const SLOT_CAP: usize> Default for TimerWheel<SLOTS, SLOT_CAP> {
    fn default() -> Self {
        Self::new()
    }
}
