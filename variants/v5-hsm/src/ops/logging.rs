//! defmt logging stub — gated by the `logging-defmt` flag.
//!
//! Driver contract (spec §Ops Features): defmt logging over UART/USB-JTAG,
//! plus the diagnostics reporting the scaffold's minimal panic handler
//! defers to it. The stub runs the shared two-state lifecycle.

use crate::events::{SIG_COMPLETE, SIG_ENGAGE, Signal};
use crate::stubkit::{self, HsmMachine, StubCtx};

/// Driver-domain signal: a log record was emitted. Consumed by the driver
/// PR (which owns the actual defmt transport).
pub const SIG_LOG_LINE: Signal = Signal::new(340);

/// Dispatcher position: ops deliver last.
pub const PRIORITY: u8 = 34;

pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_COMPLETE];

pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("logging-defmt", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::events::Event;
    use crate::fsm::Ctx;

    #[test]
    fn logging_stub_machine_engages_and_completes() {
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
