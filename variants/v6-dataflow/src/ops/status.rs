//! Status heartbeat node — gated by the `ops-status` flag (on by default;
//! both the HTTP and MQTT contracts expect status reporting).
//!
//! Driver contract (spec §Ops features): status heartbeats + boot
//! lifecycle. HTTP path: one object per heartbeat to `/v1/sensors/status`
//! (`status_id`, `subsystem`, `result`, `message`). MQTT path: a retained
//! `Status` with `Online` on every connect, `Offline` on clean disconnect,
//! LWT `Died`. In V6 the heartbeat rides the routing path like a sensor
//! emission: it produces a `status` measurement on its period and the
//! graph routes it to every compiled transport.

use crate::measurement::Measurement;
use crate::nodekit::{self, MeasRing};

/// Ingest type name on the wire (HTTP path `/v1/sensors/status`, MQTT
/// topic suffix `…/data/status`).
pub const INGEST_TYPE: &str = "status";

/// Heartbeat period in graph steps (the scaffold has no wall clock yet;
/// the runtime PR drives steps from the tick timer).
pub const PERIOD_STEPS: u32 = 100;

/// `subsystem` value for firmware-heartbeat status objects (0 = device
/// firmware; string rendering is the transport PR's fixed-buffer job).
pub const SUBSYSTEM: u8 = 0;

/// `result` value for a healthy heartbeat (0 = ok, matching the server's
/// status result convention).
pub const RESULT_OK: u8 = 0;

/// The status heartbeat node: emits a `status` measurement periodically.
pub struct Node {
    /// Heartbeats waiting for a transport drain.
    pub out: MeasRing,
    steps: u32,
    beats: u64,
    emitted: u64,
}

impl Default for Node {
    fn default() -> Self {
        Self::new()
    }
}

impl Node {
    pub const fn new() -> Self {
        Self {
            out: MeasRing::new(),
            steps: 0,
            beats: 0,
            emitted: 0,
        }
    }

    /// Heartbeats emitted since boot.
    pub fn beats(&self) -> u64 {
        self.beats
    }

    /// Measurements accepted into the output ring since boot.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Static footprint of this node (state + ring), for budget accounting.
    pub const BYTES: usize = core::mem::size_of::<Node>();

    /// One stage step: emit a heartbeat every `PERIOD_STEPS` steps.
    /// Returns 1 when a heartbeat was emitted this step.
    pub fn poll(&mut self) -> usize {
        self.steps = self.steps.wrapping_add(1);
        if !self.steps.is_multiple_of(PERIOD_STEPS) {
            return 0;
        }
        self.beats += 1;
        let m = Measurement::Status {
            subsystem: SUBSYSTEM,
            result: RESULT_OK,
        };
        if nodekit::offer(&self.out, m) {
            self.emitted += 1;
            1
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_on_the_period() {
        let mut node = Node::new();
        // The first beat lands exactly on step PERIOD_STEPS.
        for _ in 1..PERIOD_STEPS {
            assert_eq!(node.poll(), 0);
        }
        assert_eq!(node.poll(), 1);
        assert_eq!(node.beats(), 1);
        assert_eq!(
            node.out.pop(),
            Some(Measurement::Status {
                subsystem: SUBSYSTEM,
                result: RESULT_OK,
            })
        );
    }

    #[test]
    fn count_of_beats_tracks_period_completions() {
        let mut node = Node::new();
        let steps = PERIOD_STEPS * 3;
        let mut beats = 0;
        for _ in 0..steps {
            beats += node.poll();
        }
        assert_eq!(beats, 3);
        assert_eq!(node.beats(), 3);
        assert_eq!(node.emitted(), 3);
    }
}
