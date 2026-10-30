//! Decibel subsystem stub — gated by the `sensor-decibel` flag.
//!
//! Driver contract (spec §Sensor Expansion + implementation_notes/decibel.md
//! in the source monorepo): SPK0641HT4H-1 PDM mic on I2S0 at 48 kHz, A/C
//! weighting, LAeq/LCeq with Fast extrema, overload detection, per-unit
//! calibration. The driver PR replaces this stub; the stub runs the shared
//! two-state lifecycle (see [`crate::stubkit`]).

use crate::events::{SIG_COMPLETE, SIG_ENGAGE, Signal};
use crate::stubkit::{self, HsmMachine, StubCtx};

/// Driver-domain signal: a measurement window is complete and a decibel
/// report is due. The stub ignores it (lifecycle only); the driver consumes.
pub const SIG_DECIBEL_WINDOW: Signal = Signal::new(100);

/// Dispatcher position: sensors deliver before transports and ops.
pub const PRIORITY: u8 = 10;

pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_COMPLETE];

pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("decibel", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::events::Event;
    use crate::fsm::Ctx;

    #[test]
    fn decibel_stub_machine_engages_and_completes() {
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
