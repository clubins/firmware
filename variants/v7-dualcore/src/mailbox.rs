//! Control mailbox — the producer-side half of the core 0 → core 1
//! command channel.
//!
//! The channel itself is a small SPSC queue
//! ([`CONTROL_Q_CAP`](crate::boundary::CONTROL_Q_CAP) slots); what makes it
//! a mailbox rather than a backlog is the discipline implemented here,
//! which lives entirely on core 0: at most one command is outstanding at a
//! time. A new intent while a previous command is unacked does NOT queue
//! behind it — it coalesces at the producer (the previous intent stands
//! until acked). Core 1 observes commands exactly once via its consumer
//! half and acks each; every ack releases the mailbox for the next
//! command. The sensing loop is never blocked by or aware of this
//! arbitration.

/// The mailbox state — one bool, owned exclusively by core 0's system (the
/// only core that commands core 1).
#[derive(Debug)]
pub struct ControlMailbox {
    outstanding: bool,
}

impl ControlMailbox {
    pub const fn new() -> Self {
        ControlMailbox { outstanding: false }
    }

    /// Whether a command may be posted right now.
    pub const fn can_post(&self) -> bool {
        !self.outstanding
    }

    /// Marks a command as posted (the queue accepted it).
    pub fn mark_posted(&mut self) {
        self.outstanding = true;
    }

    /// Releases the mailbox when core 0 drains an ack (acks are 1:1 with
    /// commands, so any ack releases the slot).
    pub fn on_ack(&mut self) {
        self.outstanding = false;
    }

    /// Whether a command is awaiting core 1's ack.
    pub const fn is_outstanding(&self) -> bool {
        self.outstanding
    }
}

impl Default for ControlMailbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boundary::{Ack, CONTROL_Q_CAP, Control, leak_split};

    #[test]
    fn a_posted_command_is_outstanding_until_acked() {
        let (mut core1, mut core0) = leak_split();
        let mut mailbox = ControlMailbox::new();
        assert!(mailbox.can_post());

        assert!(core0.post_control(Control::ParkSampling));
        mailbox.mark_posted();
        assert!(mailbox.is_outstanding());
        assert!(!mailbox.can_post());

        // Core 1 takes the command exactly once and acks; core 0 drains
        // the ack and the mailbox is free again.
        assert_eq!(core1.drain_control(), Some(Control::ParkSampling));
        core1.post_ack(Ack::Parked);
        assert!(core0.drain_acks().next().is_some());
        mailbox.on_ack();
        assert!(!mailbox.is_outstanding());
        assert!(mailbox.can_post());
    }

    #[test]
    fn an_unacked_intent_coalesces_at_the_producer() {
        let (mut core1, mut core0) = leak_split();
        let mut mailbox = ControlMailbox::new();

        assert!(core0.post_control(Control::SetCalibrationDb(1.0)));
        mailbox.mark_posted();

        // While the first command is unacked, the discipline says HOLD:
        // the producer checks can_post() and keeps its new intent for the
        // next window instead of queueing it behind the unacked one.
        assert!(!mailbox.can_post());

        // Core 1 observes exactly the outstanding command — never a stale
        // pile — and every ack releases the mailbox.
        assert_eq!(core1.drain_control(), Some(Control::SetCalibrationDb(1.0)));
        core1.post_ack(Ack::CalibrationSet);
        assert!(core0.drain_acks().next().is_some());
        mailbox.on_ack();
        assert!(mailbox.can_post());
        assert!(core0.post_control(Control::SetCalibrationDb(2.0)));
    }

    #[test]
    fn the_channel_itself_never_grows_into_a_backlog() {
        let (mut _core1, mut core0) = leak_split();
        // Direct queue semantics (bypassing the discipline): capacity 2,
        // then reject — the queue is the floor, the mailbox is the policy.
        for _ in 0..CONTROL_Q_CAP {
            assert!(core0.post_control(Control::Shutdown));
        }
        assert!(!core0.post_control(Control::Shutdown));
    }
}
