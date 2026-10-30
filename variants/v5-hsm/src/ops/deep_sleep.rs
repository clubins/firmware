//! Deep-sleep stub — gated by the `deep-sleep` flag.
//!
//! Driver contract (spec §Ops Features): duty-cycled sampling that trades
//! measurement continuity for power. In the V5 architecture deep sleep is a
//! *top-level system state* (see [`crate::system`]); this stub is the
//! subsystem that decides when to request the transition (`SIG_SLEEP`) and
//! re-arms sensors on `SIG_WAKE`. The stub runs the shared two-state
//! lifecycle.

use crate::events::{SIG_COMPLETE, SIG_ENGAGE, Signal};
use crate::stubkit::{self, HsmMachine, StubCtx};

/// Driver-domain signal: the duty cycle decided it is time to sleep.
/// Consumed by the driver PR.
pub const SIG_SLEEP_DUE: Signal = Signal::new(330);

/// Dispatcher position: ops deliver last.
pub const PRIORITY: u8 = 33;

pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_COMPLETE];

pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("deep-sleep", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::events::Event;
    use crate::fsm::Ctx;

    #[test]
    fn deep_sleep_stub_machine_engages_and_completes() {
        let mut m = machine();
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        assert_eq!(m.hsm.current(), stubkit::STUB_ACTIVE);
        assert_eq!(m.data.engaged, 1);
        m.handle(&Event::new(SIG_COMPLETE), &mut out);
        assert_eq!(m.hsm.current(), stubkit::STUB_IDLE);
        assert_eq!(m.data.completed, 1);
        assert_eq!(m.hsm.deferred_len(), 0);
    }
}
