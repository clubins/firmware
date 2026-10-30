//! mTLS stub — gated by the `transport-mtls` flag (pulls `transport-mqtt`).
//!
//! Driver contract (spec §Flag Matrix): TLS client certificate and key come
//! from NVS; the certificate CN is the sensor UUID. This stub runs the
//! shared two-state lifecycle and stands in for the TLS-session wiring the
//! driver PR adds on top of the MQTT machine.

use crate::events::{SIG_COMPLETE, SIG_ENGAGE, Signal};
use crate::stubkit::{self, HsmMachine, StubCtx};

/// Driver-domain signal: TLS credentials were (re)loaded from NVS.
/// Consumed by the driver PR.
pub const SIG_MTLS_READY: Signal = Signal::new(215);

/// Dispatcher position: slots after the MQTT transport.
pub const PRIORITY: u8 = 22;

pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_COMPLETE];

pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("mtls", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::events::Event;
    use crate::fsm::Ctx;

    #[test]
    fn mtls_stub_machine_engages_and_completes() {
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
