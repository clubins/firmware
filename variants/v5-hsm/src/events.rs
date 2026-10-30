//! Event model: signals, scalar parameters, and well-known system signals.
//!
//! Signals are QP-style numeric identifiers. Every subsystem allocates named
//! constants from disjoint ranges so independently developed modules never
//! collide:
//!
//! | range     | owner                                        |
//! |-----------|----------------------------------------------|
//! | 0         | [`Signal::INIT`] — framework-reserved        |
//! | 1..50     | cross-cutting signals (defined here)         |
//! | 50..99    | stub plumbing (`crate::stub`)                |
//! | 100..199  | sensor capabilities, one block per driver    |
//! | 200..299  | transports (http 200s, mqtt 210s, mtls 215s) |
//! | 300..399  | ops capabilities                             |

use core::fmt;

/// Signal identifier. The framework reserves [`Signal::INIT`]; domains
/// allocate named constants from [`Signal::new`] within their range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Signal(u16);

impl Signal {
    /// Entry pseudo-event. The framework dispatches `INIT` to a state right
    /// after it is entered; a handler that answers `INIT` with
    /// [`crate::fsm::Outcome::Transit`] performs the state's initial
    /// (nested-entry) transition.
    pub const INIT: Signal = Signal(0);

    pub const fn new(raw: u16) -> Self {
        Signal(raw)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "signal {}", self.0)
    }
}

/// Event parameter. QP carries one scalar per event; these variants cover
/// the domain's scalar measurements. Structured measurement payloads arrive
/// with the driver tasks (later DAG waves) as new variants here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Param {
    None,
    U32(u32),
    I32(i32),
    U64(u64),
    F32(f32),
    F64(f64),
}

/// One event: a signal plus its parameter. `Copy` so events can be queued,
/// deferred, and re-dispatched without allocation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    pub signal: Signal,
    pub param: Param,
}

impl Event {
    pub const fn new(signal: Signal) -> Self {
        Event {
            signal,
            param: Param::None,
        }
    }

    pub const fn with_param(signal: Signal, param: Param) -> Self {
        Event { signal, param }
    }
}

impl Event {
    /// The framework's entry pseudo-event.
    pub const INIT: Event = Event {
        signal: Signal::INIT,
        param: Param::None,
    };
}

// Cross-cutting signals, shared by the system machine and every stub.
// Feature-specific constants live in their own modules, one block per flag.
pub const SIG_TICK: Signal = Signal::new(1);
pub const SIG_MEAS_READY: Signal = Signal::new(2);
pub const SIG_WIFI_UP: Signal = Signal::new(3);
pub const SIG_WIFI_DOWN: Signal = Signal::new(4);
pub const SIG_PUBLISH_OK: Signal = Signal::new(5);
pub const SIG_PUBLISH_FAIL: Signal = Signal::new(6);
pub const SIG_SLEEP: Signal = Signal::new(7);
pub const SIG_WAKE: Signal = Signal::new(8);
pub const SIG_STATUS_HEARTBEAT: Signal = Signal::new(9);
/// Stub lifecycle: engage a capability machine (enter its active state).
pub const SIG_ENGAGE: Signal = Signal::new(10);
/// Stub lifecycle: the active capability finished its (stub) work.
pub const SIG_COMPLETE: Signal = Signal::new(11);
/// A subsystem reported an unrecoverable condition; the system machine
/// latches into its fault state from any depth.
pub const SIG_FAULT: Signal = Signal::new(12);
/// Bring-up finished (self-test, NVS read, provisioning check): the system
/// machine leaves BOOT and enters the RUNNING composite. Initial transitions
/// only descend, so boot exits through a normal event — the QP rule.
pub const SIG_BOOT_DONE: Signal = Signal::new(13);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_is_copy_and_comparable() {
        let a = Event::with_param(SIG_TICK, Param::U32(7));
        let b = a;
        assert_eq!(a, b);
        assert_eq!(b.param, Param::U32(7));
        assert_eq!(b.signal.raw(), 1);
    }

    #[test]
    fn init_is_reserved_zero() {
        assert_eq!(Signal::INIT.raw(), 0);
        assert_eq!(Event::INIT.signal, Signal::INIT);
    }
}
