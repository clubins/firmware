//! The sensors' clock abstraction — the time half of the mechanism the
//! drivers run over. Shared by every transport (I2C and UART alike),
//! which is why it lives behind `any(driver-i2c, driver-uart)` rather
//! than inside [`crate::sensors::bus`].
//!
//! V5 is event-driven: machines never block, so *pacing* lives in the
//! driver's state machine (poll on `SIG_TICK`, never `delay`), against
//! this millisecond clock. The target adapter wraps `esp-hal`'s
//! monotonic time; host tests script it with [`SharedClock`].

/// Milliseconds since boot — the pacing source for datasheet timings
/// (conversion durations, frame waits). The target adapter wraps
/// `esp-hal`'s monotonic time; host tests script it with [`SharedClock`].
pub trait Clock {
    fn now_ms(&self) -> u64;
}

/// A clock the test advances directly, so a flow can move time between
/// dispatches without sleeping. The driver owns a `Box<dyn Clock>` over a
/// clone of the same handle — `Rc` makes the clones share one clock.
#[derive(Clone, Debug, Default)]
pub struct SharedClock(alloc::rc::Rc<core::cell::Cell<u64>>);

impl SharedClock {
    pub fn new(ms: u64) -> Self {
        Self(alloc::rc::Rc::new(core::cell::Cell::new(ms)))
    }

    pub fn set(&self, ms: u64) {
        self.0.set(ms);
    }

    pub fn advance(&self, ms: u64) {
        self.0.set(self.0.get() + ms);
    }
}

impl Clock for SharedClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}
