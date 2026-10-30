//! MQTT v2 transport stub — gated by the `transport-mqtt` flag (pulls
//! `net-wifi` and `proto-v2`).
//!
//! Driver contract (spec §Transports + api/mqtt/topics.md in the source
//! monorepo): publish a retained `Status` (`Online`) on every connect, one
//! message per measurement on `v2/sensors/<uuid>/data/<type>` at QoS 1,
//! `seq` from 0 per topic per boot, `boot_id` stable within a boot, clean
//! disconnect publishes `Offline`, LWT `Died`. The stub runs the shared
//! two-state lifecycle.

use crate::events::{SIG_COMPLETE, SIG_ENGAGE, Signal};
use crate::stubkit::{self, HsmMachine, StubCtx};

/// Driver-domain signal: a protobuf measurement is ready to publish.
/// Consumed by the driver PR.
pub const SIG_MQTT_PUBLISH: Signal = Signal::new(210);

/// Dispatcher position: transports deliver after sensors, before ops.
pub const PRIORITY: u8 = 21;

pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_COMPLETE];

pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("mqtt", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::events::Event;
    use crate::fsm::Ctx;

    #[test]
    fn mqtt_stub_machine_engages_and_completes() {
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
