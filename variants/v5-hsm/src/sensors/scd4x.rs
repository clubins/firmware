//! SCD4x stub — gated by the `sensor-scd4x` flag.
//!
//! Driver contract (spec §Sensor Expansion): I2C CO₂ → the `co2` ingest
//! type (new measurement type; the firmware emits it per the
//! one-topic-per-type rule even before the server grows a receiver).
//! The stub runs the shared two-state lifecycle.

use crate::events::{SIG_COMPLETE, SIG_ENGAGE, Signal};
use crate::stubkit::{self, HsmMachine, StubCtx};

/// Driver-domain signal: a periodic CO₂ measurement is due. Consumed by the
/// driver PR.
pub const SIG_SCD4X_MEASURE: Signal = Signal::new(140);

/// Dispatcher position: sensors deliver before transports and ops.
pub const PRIORITY: u8 = 14;

pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_COMPLETE];

pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("scd4x", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::events::Event;
    use crate::fsm::Ctx;

    #[test]
    fn scd4x_stub_machine_engages_and_completes() {
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
