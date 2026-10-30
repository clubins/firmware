//! Shared machinery for the feature-gated capability stubs.
//!
//! Each capability flag (`sensor-*`, `transport-*`, ops) compiles in a small
//! subsystem machine so the flag demonstrably gates *something wired into
//! the architecture* — not dead code. Until the driver PRs land
//! (docs/plan/build-dag.md), every subsystem runs the same two-state
//! lifecycle: IDLE → ACTIVE on `SIG_ENGAGE`, ACTIVE → IDLE on
//! `SIG_COMPLETE` (which posts `SIG_MEAS_READY`). Per-capability signal
//! constants and subscriptions live in the stub modules; the lifecycle shape
//! is deliberately shared, because the scaffold's contract is "the framework
//! works and every flag compiles independently", not "drivers exist".

pub use crate::dispatcher::{HsmMachine, Machine};
use crate::events::{Event, Signal};
use crate::fsm::{Ctx, Outcome, StateDef, StateId};

/// Defer-queue capacity for stub machines. The dispatcher's queue absorbs
/// bursts; stubs defer nothing unless a test does.
pub const STUB_DEFER_CAP: usize = 2;

/// Context carried by every stub machine: how many engage/complete cycles
/// ran. Kept observable so stub tests assert real movement, not just
/// "no panic".
#[derive(Debug, Default)]
pub struct StubCtx {
    pub engaged: u32,
    pub completed: u32,
}

/// The stub root state.
pub const STUB_IDLE: StateId = 0;
/// The stub's active (engaged) state.
pub const STUB_ACTIVE: StateId = 1;

fn idle_handler(ctx: &mut StubCtx, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == crate::events::SIG_ENGAGE => {
            ctx.engaged += 1;
            Outcome::Transit(STUB_ACTIVE)
        }
        _ => Outcome::Unhandled,
    }
}

fn active_handler(ctx: &mut StubCtx, out: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == crate::events::SIG_COMPLETE => {
            ctx.completed += 1;
            out.post(Event::new(crate::events::SIG_MEAS_READY));
            Outcome::Transit(STUB_IDLE)
        }
        _ => Outcome::Unhandled,
    }
}

/// Shared two-state lifecycle table. `static` because every machine of this
/// shape is identical until the driver PRs specialize it.
pub static STUB_TABLE: &[StateDef<StubCtx>] = &[
    StateDef {
        name: "stub:idle",
        parent: None,
        entry: None,
        exit: None,
        handler: idle_handler,
        transitions: &[],
    },
    StateDef {
        name: "stub:active",
        parent: Some(STUB_IDLE),
        entry: None,
        exit: None,
        handler: active_handler,
        transitions: &[],
    },
];

/// Builds a stub machine for a capability. `subscriptions` are the signals
/// the real driver will subscribe to; the stub reacts only to the shared
/// engage/complete pair, so wiring compiles unchanged when drivers land.
pub fn machine(
    id: &'static str,
    priority: u8,
    subscriptions: &'static [Signal],
) -> HsmMachine<StubCtx> {
    HsmMachine::new(
        id,
        priority,
        subscriptions,
        STUB_TABLE,
        STUB_IDLE,
        STUB_DEFER_CAP,
        StubCtx::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{SIG_COMPLETE, SIG_ENGAGE};

    #[test]
    fn stub_machine_cycles_between_idle_and_active() {
        let mut m = machine("test-stub", 5, &[SIG_ENGAGE, SIG_COMPLETE]);
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        m.handle(&Event::new(SIG_ENGAGE), &mut out);
        assert_eq!(m.hsm.current(), STUB_ACTIVE);
        assert_eq!(m.data.engaged, 1);
        m.handle(&Event::new(SIG_COMPLETE), &mut out);
        assert_eq!(m.hsm.current(), STUB_IDLE);
        assert_eq!(m.data.completed, 1);
        assert_eq!(m.hsm.deferred_len(), 0);
    }
}
