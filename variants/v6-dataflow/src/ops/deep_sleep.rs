//! Deep-sleep node — gated by the `deep-sleep` flag.
//!
//! Driver contract (spec §Ops features): duty-cycled sampling between
//! sleeps, trading measurement continuity for power. The sleep/rtc-wake
//! machinery lands with the ops PRs; the scaffold keeps the node in the
//! graph (its RAM is budgeted) and advances a tick counter per pump step.

/// The deep-sleep node: present, budgeted, inert until the ops PR.
pub struct Node {
    ticks: u32,
}

impl Node {
    pub const fn new() -> Self {
        Self { ticks: 0 }
    }

    /// Pump steps observed since boot.
    pub fn ticks(&self) -> u32 {
        self.ticks
    }

    /// Static footprint of this node, for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step (no data — deep sleep has no measurement path).
    pub fn poll(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_advance_per_poll() {
        let mut node = Node::new();
        node.poll();
        assert_eq!(node.ticks(), 1);
    }
}
