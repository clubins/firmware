//! Jitter accounting — the executive's honesty ledger.
//!
//! The time-triggered guarantee is *bounded* jitter: every phase runs within a
//! stated number of ticks of its scheduled slot. This tracker records actual
//! lateness per phase so tests (and on-device diagnostics) can assert the
//! bound instead of trusting it.

use super::phase::MAX_PHASES;

#[derive(Clone, Copy, Debug, Default)]
pub struct JitterTracker {
    last: [u32; MAX_PHASES],
    max: [u32; MAX_PHASES],
    sum: [u64; MAX_PHASES],
    samples: [u32; MAX_PHASES],
}

impl JitterTracker {
    /// Record that the phase at `slot` ran `lateness_ticks` after its
    /// scheduled grid slot. Zero means on time.
    pub fn record(&mut self, slot: usize, lateness_ticks: u32) {
        self.last[slot] = lateness_ticks;
        self.max[slot] = self.max[slot].max(lateness_ticks);
        self.sum[slot] += lateness_ticks as u64;
        self.samples[slot] = self.samples[slot].saturating_add(1);
    }

    /// Lateness of the most recent run.
    pub fn last(&self, slot: usize) -> u32 {
        self.last[slot]
    }

    /// Worst lateness ever recorded for the phase.
    pub fn max(&self, slot: usize) -> u32 {
        self.max[slot]
    }

    /// Mean lateness, in ticks (truncated; 0 before the first sample).
    pub fn mean(&self, slot: usize) -> u64 {
        self.sum[slot]
            .checked_div(self.samples[slot] as u64)
            .unwrap_or(0)
    }

    pub fn samples(&self, slot: usize) -> u32 {
        self.samples[slot]
    }

    /// Worst lateness across all phases.
    pub fn max_overall(&self) -> u32 {
        self.max.iter().copied().max().unwrap_or(0)
    }

    /// The first-class guarantee: did every phase stay within `bound` ticks
    /// of its scheduled slot?
    pub fn within_bound(&self, bound: u32) -> bool {
        self.max_overall() <= bound
    }
}
