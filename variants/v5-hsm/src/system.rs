//! The system machine — V5's top-level FSM and the composition root.
//!
//! Deep sleep is a *top-level system state* (the V5 architecture's
//! signature choice): entering `SYS_SLEEPING` exits the whole
//! `RUNNING` composite (sampling/publishing), and waking re-enters it
//! through its initial transition. Faults latch from any depth via the
//! root's catch-all `SIG_FAULT` row — the hardware watchdog (always on,
//! not a flag) resets the device out of `SYS_FAULT` in real bring-up.
//!
//! The system machine is the one machine with a hand-written [`Machine`]
//! impl: after each event it drains its defer queue while the link is up,
//! which is how offline publish failures get retried on reconnect
//! (defer-on-fail, recall-on-reconnect).

use alloc::vec::Vec;

use crate::dispatcher::{Dispatcher, Machine};
use crate::events::{
    Event, SIG_BOOT_DONE, SIG_FAULT, SIG_MEAS_READY, SIG_PUBLISH_FAIL, SIG_PUBLISH_OK,
    SIG_SLEEP, SIG_STATUS_HEARTBEAT, SIG_WAKE, SIG_WIFI_DOWN, SIG_WIFI_UP, Signal,
};
use crate::fsm::{Ctx, Hsm, Outcome, StateDef, StateId};

pub const SYS_ROOT: StateId = 0;
pub const SYS_BOOT: StateId = 1;
pub const SYS_RUNNING: StateId = 2;
pub const SYS_SAMPLING: StateId = 3;
pub const SYS_PUBLISHING: StateId = 4;
pub const SYS_SLEEPING: StateId = 5;
pub const SYS_FAULT: StateId = 6;

/// Context of the system machine. `trace` records entry/exit order so host
/// tests can pin the nesting semantics.
#[derive(Debug, Default)]
pub struct SCtx {
    pub online: bool,
    pub publish_attempts: u32,
    pub trace: Vec<&'static str>,
}

fn sys_entry(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("enter:sys");
}

/// Root handler: link state and faults are system-level — they apply from
/// any depth, whichever leaf is active.
fn sys_handler(d: &mut SCtx, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        // The system machine's initial state is BOOT.
        Signal::INIT => Outcome::Transit(SYS_BOOT),
        s if s == SIG_FAULT => Outcome::Transit(SYS_FAULT),
        s if s == SIG_WIFI_UP => {
            d.online = true;
            Outcome::Handled
        }
        s if s == SIG_WIFI_DOWN => {
            d.online = false;
            Outcome::Handled
        }
        _ => Outcome::Unhandled,
    }
}

fn boot_entry(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("enter:boot");
}

fn boot_handler(_: &mut SCtx, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        // BOOT is a leaf: no initial transition of its own.
        Signal::INIT => Outcome::Handled,
        // Bring-up completes as an event, not an initial transition —
        // initial transitions only descend the hierarchy (QP rule).
        s if s == SIG_BOOT_DONE => Outcome::Transit(SYS_RUNNING),
        _ => Outcome::Unhandled,
    }
}

fn running_entry(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("enter:running");
}

fn running_exit(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("exit:running");
}

fn running_handler(_: &mut SCtx, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Transit(SYS_SAMPLING),
        // Top-level transition to the sleep state exits the whole composite.
        s if s == SIG_SLEEP => Outcome::Transit(SYS_SLEEPING),
        s if s == SIG_WAKE => Outcome::Handled, // spurious wake while awake
        _ => Outcome::Unhandled,
    }
}

fn sampling_entry(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("enter:sampling");
}

fn sampling_exit(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("exit:sampling");
}

fn sampling_handler(_: &mut SCtx, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_MEAS_READY => Outcome::Transit(SYS_PUBLISHING),
        _ => Outcome::Unhandled,
    }
}

fn publishing_entry(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("enter:publishing");
}

fn publishing_handler(d: &mut SCtx, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_PUBLISH_FAIL => {
            d.publish_attempts += 1;
            if d.online {
                // The transport is up; the retry itself is the transport's
                // job — the system state stays in publishing.
                Outcome::Handled
            } else {
                // Offline: park the failure and retry on reconnect
                // (the SystemMachine impl recalls on SIG_WIFI_UP).
                Outcome::Defer
            }
        }
        s if s == SIG_PUBLISH_OK => Outcome::Transit(SYS_SAMPLING),
        _ => Outcome::Unhandled,
    }
}

fn sleeping_entry(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("enter:sleeping");
}

fn sleeping_exit(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("exit:sleeping");
}

fn sleeping_handler(_: &mut SCtx, _: &mut Ctx, event: &Event) -> Outcome {
    match event.signal {
        Signal::INIT => Outcome::Handled,
        s if s == SIG_WAKE => Outcome::Transit(SYS_RUNNING),
        s if s == SIG_SLEEP => Outcome::Handled, // already asleep
        _ => Outcome::Unhandled,
    }
}

fn fault_entry(d: &mut SCtx, _: &mut Ctx) {
    d.trace.push("enter:fault");
}

fn fault_handler(_: &mut SCtx, _: &mut Ctx, _: &Event) -> Outcome {
    // Latched: swallow everything until the (unconditional) hardware
    // watchdog resets the device.
    Outcome::Handled
}

pub static SYSTEM_TABLE: &[StateDef<SCtx>] = &[
    StateDef {
        name: "sys",
        parent: None,
        entry: Some(sys_entry),
        exit: None,
        handler: sys_handler,
        transitions: &[],
    },
    StateDef {
        name: "boot",
        parent: Some(SYS_ROOT),
        entry: Some(boot_entry),
        exit: None,
        handler: boot_handler,
        transitions: &[],
    },
    StateDef {
        name: "running",
        parent: Some(SYS_ROOT),
        entry: Some(running_entry),
        exit: Some(running_exit),
        handler: running_handler,
        transitions: &[],
    },
    StateDef {
        name: "sampling",
        parent: Some(SYS_RUNNING),
        entry: Some(sampling_entry),
        exit: Some(sampling_exit),
        handler: sampling_handler,
        transitions: &[],
    },
    StateDef {
        name: "publishing",
        parent: Some(SYS_RUNNING),
        entry: Some(publishing_entry),
        exit: None,
        handler: publishing_handler,
        transitions: &[],
    },
    StateDef {
        name: "sleeping",
        parent: Some(SYS_ROOT),
        entry: Some(sleeping_entry),
        exit: Some(sleeping_exit),
        handler: sleeping_handler,
        transitions: &[],
    },
    StateDef {
        name: "fault",
        parent: Some(SYS_ROOT),
        entry: Some(fault_entry),
        exit: None,
        handler: fault_handler,
        transitions: &[],
    },
];

/// Signals the system machine subscribes to on the dispatcher.
pub static SYSTEM_SUBSCRIPTIONS: &[Signal] = &[
    Signal::INIT,
    SIG_BOOT_DONE,
    SIG_MEAS_READY,
    SIG_PUBLISH_OK,
    SIG_PUBLISH_FAIL,
    SIG_WIFI_UP,
    SIG_WIFI_DOWN,
    SIG_SLEEP,
    SIG_WAKE,
    SIG_STATUS_HEARTBEAT,
];

/// The system machine packaged for the dispatcher, with post-dispatch
/// recall of deferred events while the link is up.
pub struct SystemMachine {
    pub hsm: Hsm<SCtx>,
    pub data: SCtx,
}

impl SystemMachine {
    pub fn new() -> Self {
        SystemMachine {
            hsm: Hsm::new(SYSTEM_TABLE, SYS_ROOT, 8),
            data: SCtx::default(),
        }
    }
}

impl Default for SystemMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine for SystemMachine {
    fn id(&self) -> &'static str {
        "system"
    }

    fn priority(&self) -> u8 {
        0 // the system machine sees every event first
    }

    fn subscribes(&self, signal: Signal) -> bool {
        SYSTEM_SUBSCRIPTIONS.contains(&signal)
    }

    fn handle(&mut self, event: &Event, ctx: &mut Ctx) {
        self.hsm.dispatch(&mut self.data, ctx, event);
        // Reconnect recall: while the link is up, re-deliver parked events
        // (bounded by the queue length at entry — a recall that defers
        // again must not spin).
        if self.data.online && self.hsm.deferred_len() > 0 {
            let mut budget = self.hsm.deferred_len();
            while budget > 0 && self.hsm.deferred_len() > 0 {
                budget -= 1;
                if !self.hsm.recall(&mut self.data, ctx) {
                    break;
                }
            }
        }
    }

    fn start(&mut self, ctx: &mut Ctx) {
        self.hsm.start(&mut self.data, ctx);
        // Stub bring-up: real bring-up (NVS read, self-test, provisioning)
        // posts this when it finishes; the scaffold's stub completes
        // immediately so the device loop reaches RUNNING.
        ctx.post(Event::new(SIG_BOOT_DONE));
    }
}

/// Number of machines `build` registers under the current feature set
/// (the system machine plus one stub per enabled public flag).
pub fn expected_machine_count() -> usize {
    let mut n = 1;
    if cfg!(feature = "sensor-decibel") {
        n += 1;
    }
    if cfg!(feature = "sensor-pax") {
        n += 1;
    }
    if cfg!(feature = "sensor-bme280") {
        n += 1;
    }
    if cfg!(feature = "sensor-sht4x") {
        n += 1;
    }
    if cfg!(feature = "sensor-scd4x") {
        n += 1;
    }
    if cfg!(feature = "sensor-pms5003") {
        n += 1;
    }
    if cfg!(feature = "sensor-bh1750") {
        n += 1;
    }
    if cfg!(feature = "transport-http") {
        n += 1;
    }
    if cfg!(feature = "transport-mqtt") {
        n += 1;
    }
    if cfg!(feature = "transport-mtls") {
        n += 1;
    }
    if cfg!(feature = "ota") {
        n += 1;
    }
    if cfg!(feature = "provisioning") {
        n += 1;
    }
    if cfg!(feature = "deep-sleep") {
        n += 1;
    }
    if cfg!(feature = "logging-defmt") {
        n += 1;
    }
    if cfg!(feature = "ops-status") {
        n += 1;
    }
    n
}

/// Composition root: the system machine plus every feature-gated stub.
/// The dispatcher owns all machines; nothing else holds references.
pub fn build() -> Dispatcher {
    let mut d = Dispatcher::new(64);
    d.register(alloc::boxed::Box::new(SystemMachine::new()));

    #[cfg(feature = "sensor-decibel")]
    d.register(alloc::boxed::Box::new(crate::sensors::decibel::machine()));
    #[cfg(feature = "sensor-pax")]
    d.register(alloc::boxed::Box::new(crate::sensors::pax::machine()));
    #[cfg(feature = "sensor-bme280")]
    d.register(alloc::boxed::Box::new(crate::sensors::bme280::machine()));
    #[cfg(feature = "sensor-sht4x")]
    d.register(alloc::boxed::Box::new(crate::sensors::sht4x::machine()));
    #[cfg(feature = "sensor-scd4x")]
    d.register(alloc::boxed::Box::new(crate::sensors::scd4x::machine()));
    #[cfg(feature = "sensor-pms5003")]
    d.register(alloc::boxed::Box::new(crate::sensors::pms5003::machine()));
    #[cfg(feature = "sensor-bh1750")]
    d.register(alloc::boxed::Box::new(crate::sensors::bh1750::machine()));

    #[cfg(feature = "transport-http")]
    d.register(alloc::boxed::Box::new(crate::transports::http::machine()));
    #[cfg(feature = "transport-mqtt")]
    d.register(alloc::boxed::Box::new(crate::transports::mqtt::machine()));
    #[cfg(feature = "transport-mtls")]
    d.register(alloc::boxed::Box::new(crate::transports::mtls::machine()));

    #[cfg(feature = "ota")]
    d.register(alloc::boxed::Box::new(crate::ops::ota::machine()));
    #[cfg(feature = "provisioning")]
    d.register(alloc::boxed::Box::new(crate::ops::provisioning::machine()));
    #[cfg(feature = "deep-sleep")]
    d.register(alloc::boxed::Box::new(crate::ops::deep_sleep::machine()));
    #[cfg(feature = "logging-defmt")]
    d.register(alloc::boxed::Box::new(crate::ops::logging::machine()));
    #[cfg(feature = "ops-status")]
    d.register(alloc::boxed::Box::new(crate::ops::status::machine()));

    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SIG_ENGAGE;

    /// Starts the machine and completes bring-up: the dispatcher delivers
    /// `SIG_BOOT_DONE` once bring-up finishes, moving BOOT → RUNNING →
    /// SAMPLING (same flow as `start_all` on device).
    fn setup() -> SystemMachine {
        let mut m = SystemMachine::new();
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.start(&mut out);
        assert_eq!(m.hsm.current(), SYS_BOOT);
        assert!(
            m.hsm
                .dispatch(&mut m.data, &mut out, &Event::new(SIG_BOOT_DONE))
        );
        m
    }

    #[test]
    fn boot_flows_through_running_into_sampling() {
        let m = setup();
        assert_eq!(m.hsm.current(), SYS_SAMPLING);
        assert_eq!(
            m.data.trace,
            ["enter:sys", "enter:boot", "enter:running", "enter:sampling"]
        );
    }

    #[test]
    fn measurement_cycle_samples_then_publishes_then_returns() {
        let mut m = setup();
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };

        assert!(
            m.hsm
                .dispatch(&mut m.data, &mut out, &Event::new(SIG_MEAS_READY))
        );
        assert_eq!(m.hsm.current(), SYS_PUBLISHING);
        assert!(
            m.hsm
                .dispatch(&mut m.data, &mut out, &Event::new(SIG_PUBLISH_OK))
        );
        assert_eq!(m.hsm.current(), SYS_SAMPLING);
        assert_eq!(m.data.publish_attempts, 0);
    }

    #[test]
    fn offline_publish_failure_defers_and_reconnect_recalls() {
        let mut m = setup();
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };

        // Measurement ready; publish fails while offline → deferred.
        assert!(
            m.hsm
                .dispatch(&mut m.data, &mut out, &Event::new(SIG_MEAS_READY))
        );
        assert_eq!(m.hsm.current(), SYS_PUBLISHING);
        assert!(
            m.hsm
                .dispatch(&mut m.data, &mut out, &Event::new(SIG_PUBLISH_FAIL))
        );
        assert_eq!(m.data.publish_attempts, 1);
        assert_eq!(m.hsm.deferred_len(), 1);

        // Link comes up; the recall must run through Machine::handle (the
        // dispatcher's entry point), which drains the defer queue afterwards.
        let ev = Event::new(SIG_WIFI_UP);
        m.handle(&ev, &mut out);
        assert!(m.data.online);
        assert_eq!(m.hsm.deferred_len(), 0);
        assert_eq!(m.data.publish_attempts, 2);
        assert_eq!(m.hsm.stats.recalled, 1);
        assert_eq!(m.hsm.current(), SYS_PUBLISHING);
    }

    #[test]
    fn deep_sleep_is_a_top_level_state_exiting_the_whole_composite() {
        let mut m = setup();
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };
        m.data.trace.clear();

        assert!(
            m.hsm
                .dispatch(&mut m.data, &mut out, &Event::new(SIG_SLEEP))
        );
        assert_eq!(m.hsm.current(), SYS_SLEEPING);
        // Entering sleep exits sampling then running (innermost first).
        assert_eq!(
            m.data.trace[..3],
            ["exit:sampling", "exit:running", "enter:sleeping"]
        );

        m.data.trace.clear();
        assert!(m.hsm.dispatch(&mut m.data, &mut out, &Event::new(SIG_WAKE)));
        assert_eq!(m.hsm.current(), SYS_SAMPLING);
        // Wake re-enters running and follows its initial transition.
        assert_eq!(
            m.data.trace,
            ["exit:sleeping", "enter:running", "enter:sampling"]
        );
    }

    #[test]
    fn fault_latches_from_any_depth_and_swallows_traffic() {
        let mut m = setup();
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };

        assert!(m.hsm.dispatch(
            &mut m.data,
            &mut out,
            &Event::new(crate::events::SIG_FAULT)
        ));
        assert_eq!(m.hsm.current(), SYS_FAULT);
        // Latched: even measurement traffic is consumed without movement.
        assert!(
            m.hsm
                .dispatch(&mut m.data, &mut out, &Event::new(SIG_MEAS_READY))
        );
        assert_eq!(m.hsm.current(), SYS_FAULT);
        assert_eq!(m.hsm.stats.unhandled, 0);
    }

    #[test]
    fn link_down_is_benign_from_any_state() {
        let mut m = setup();
        let mut sink = |_: Event| {};
        let mut out = Ctx { post: &mut sink };

        assert!(
            m.hsm
                .dispatch(&mut m.data, &mut out, &Event::new(SIG_WIFI_DOWN))
        );
        assert!(!m.data.online);
        assert_eq!(m.hsm.stats.unhandled, 0);
        assert_eq!(m.hsm.current(), SYS_SAMPLING);
    }

    #[test]
    fn build_registers_one_machine_per_enabled_flag() {
        let d = build();
        assert_eq!(d.machine_count(), expected_machine_count());
    }

    #[test]
    fn dispatcher_delivers_engage_only_to_subscribed_stubs() {
        let mut d = build();
        d.start_all();
        // Stub bring-up queues SIG_BOOT_DONE: one step, delivered to the
        // system machine alone (stubs do not subscribe to it).
        assert_eq!(d.run_until_idle(16), 1);
        assert_eq!(d.stats.delivered, 1);
        // SIG_ENGAGE: the system machine does not subscribe; every stub does.
        assert!(d.post(Event::new(SIG_ENGAGE)));
        assert_eq!(d.run_until_idle(16), 1);
        assert_eq!(d.stats.delivered, expected_machine_count());
    }
}
