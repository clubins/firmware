//! HTTP transport stub — gated by the `transport-http` flag (pulls
//! `net-wifi`).
//!
//! Driver contract (spec §Transports): Wi-Fi STA, then HTTPS POST of one
//! JSON object per measurement to `<endpoint>/v1/sensors/<type>`, fields
//! exactly as the Go ingest structs define them (`sensor_id`, `apikey`,
//! `time`, `measurement_id`, `dba`/`dbc`, `occupancy_count`, weather
//! fields, status fields). API key from NVS only. Failures back off
//! exponentially and drain a bounded offline buffer. The stub runs the
//! shared two-state lifecycle.

use crate::events::{SIG_COMPLETE, SIG_ENGAGE, Signal};
use crate::stubkit::{self, HsmMachine, StubCtx};

/// Driver-domain signal: a measurement batch is ready to send. Consumed by
/// the driver PR.
pub const SIG_HTTP_SEND: Signal = Signal::new(200);

/// Dispatcher position: transports deliver after sensors, before ops.
pub const PRIORITY: u8 = 20;

pub const SUBSCRIPTIONS: &[Signal] = &[SIG_ENGAGE, SIG_COMPLETE];

pub fn machine() -> HsmMachine<StubCtx> {
    stubkit::machine("http", PRIORITY, SUBSCRIPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Machine;
    use crate::events::Event;
    use crate::fsm::Ctx;

    #[test]
    fn http_stub_machine_engages_and_completes() {
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
