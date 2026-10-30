//! PMS5003 stub — gated by the `sensor-pms5003` flag.
//!
//! Driver contract (spec §Sensor Expansion): UART particulate sensor → the
//! `pm25` ingest type (new measurement type; emitted per the
//! one-topic-per-type rule). The stub runs the shared two-state lifecycle.

use crate::events::{SIG_COMPLETE, SIG_ENGAGE, Signal};
use crate::stubkit::{self, HsmMachine, StubCtx};

/// Driver-domain signal: a PM2.5 reading frame is due. Consumed by the
/// driver PR.
pub const SIG_PMS5003_FRAME: Signal = Signal::new(150);

/// Dispatcher position: sensors deliver before transports and ops.
pub const PRIORITY: u8 = 15;

pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_COMPLETE];

pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("pms5003", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::events::Event;
    use crate::fsm::Ctx;

    #[test]
    fn pms5003_stub_machine_engages_and_completes() {
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
