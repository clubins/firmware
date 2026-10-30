//! The sensors' GPIO input abstraction — the level-sampling voice of the
//! PIR front-end. The I2C and UART counterparts live in
//! [`crate::sensors::bus`] and [`crate::sensors::uart`]; the same
//! never-block rule applies.
//!
//! A PIR line carries one bit with no failure channel: it is either
//! asserted (motion) or not. The driver therefore samples levels rather
//! than taking interrupts — V5's machines never block, and the target-side
//! adapter (esp-hal GPIO interrupt → queued event) arrives with the bus
//! bring-up wave (`docs/plan/build-dag.md`, `feat/driver-buses`); until
//! then the drivers are exercised on the host against
//! [`ScriptedGpio`], which replays recorded level sequences, one sample
//! per call.

/// Synchronous GPIO input — the level-sampling voice of the drivers.
/// Mirrors the embedded-hal input-pin operation set (a single level
/// sample) so the future `esp-hal` adapter is a thin wrapper. Sampling
/// never blocks and never fails: a one-wire level read has nothing to
/// error about, and a stuck line shows up as a constant level, not an
/// error report.
pub trait GpioInput {
    /// Samples the line: `true` means asserted (the PIR saw motion).
    fn level(&mut self) -> bool;
}

// ---- host-side scripted GPIO (the [`crate::sensors::bus::ScriptedBus`]
// counterpart) ----

/// A scripted GPIO line: each `level()` call hands out the next recorded
/// sample, and a drained script reads as an idle-low line — the normal
/// resting state of a PIR output, not a flow bug (the mirror of the
/// scripted UART's empty wire). `expect_done` pins that the driver
/// sampled every scripted level, in order.
#[derive(Clone, Debug)]
pub struct ScriptedGpio {
    script: alloc::collections::VecDeque<bool>,
    pub performed: alloc::vec::Vec<bool>,
}

impl ScriptedGpio {
    pub fn new(script: impl IntoIterator<Item = bool>) -> Self {
        Self {
            script: alloc::collections::VecDeque::from_iter(script),
            performed: alloc::vec::Vec::new(),
        }
    }

    /// Panics unless every recorded sample was consumed.
    pub fn expect_done(&self) {
        assert!(
            self.script.is_empty(),
            "scripted gpio did not reach the end of the recorded sequence \
             ({} sample(s) left)",
            self.script.len()
        );
    }
}

impl GpioInput for ScriptedGpio {
    fn level(&mut self) -> bool {
        // A drained script is an idle-low line — the normal state —
        // so sampling past the end reports the line low instead of
        // panicking.
        let level = self.script.pop_front().unwrap_or(false);
        self.performed.push(level);
        level
    }
}

/// A shared handle to a [`ScriptedGpio`]: the driver owns a
/// `Box<dyn GpioInput>` over a clone, while the test keeps typed access
/// to the script's consumed state. `Rc` makes the clones share one
/// scripted line.
#[derive(Clone, Debug)]
pub struct SharedGpio(alloc::rc::Rc<core::cell::RefCell<ScriptedGpio>>);

impl SharedGpio {
    pub fn new(script: impl IntoIterator<Item = bool>) -> Self {
        Self(alloc::rc::Rc::new(core::cell::RefCell::new(
            ScriptedGpio::new(script),
        )))
    }

    /// Panics unless every recorded sample was consumed.
    pub fn expect_done(&self) {
        self.0.borrow().expect_done();
    }

    /// The consumed sample log, for shape assertions.
    pub fn sampled(&self) -> alloc::vec::Vec<bool> {
        self.0.borrow().performed.clone()
    }
}

impl GpioInput for SharedGpio {
    fn level(&mut self) -> bool {
        self.0.borrow_mut().level()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_gpio_replays_samples_in_order() {
        let mut gpio = ScriptedGpio::new([false, true, true, false]);
        assert!(!gpio.level());
        assert!(gpio.level());
        assert!(gpio.level());
        assert!(!gpio.level());
        assert_eq!(gpio.performed, [false, true, true, false]);
        gpio.expect_done();
    }

    #[test]
    fn a_drained_script_is_an_idle_low_line() {
        let mut gpio = ScriptedGpio::new([true]);
        assert!(gpio.level());
        // Past the script the line rests low, and the log records it.
        assert!(!gpio.level());
        assert!(!gpio.level());
        assert_eq!(gpio.performed, [true, false, false]);
        gpio.expect_done();
    }

    #[test]
    fn shared_gpio_shares_one_script_between_clones() {
        let gpio = SharedGpio::new([false, true]);
        let mut driver_side = gpio.clone();
        assert!(!driver_side.level());
        assert!(driver_side.level());
        assert_eq!(gpio.sampled(), [false, true]);
        gpio.expect_done();
    }
}
