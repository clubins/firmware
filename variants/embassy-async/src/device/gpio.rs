//! The device GPIO front end: the PIR's interrupt line.

use esp_hal::gpio::{Input, InputConfig, Pull};

use clubins_firmware_v3::sensors::gpio::{Edge, EdgeSource};

/// The PIR front end on its interrupt pin: waits on the pin's own edge
/// interrupts — the executor parks the task until the pad actually moves,
/// nothing polls — and re-syncs the tracked level against the pad on
/// every arm, so a transition missed while the task was busy publishing
/// (the edge seam's waits are re-armed per call) surfaces as the next
/// edge instead of going stale.
pub(super) struct PirInput<'a> {
    pin: Input<'a>,
    high: bool,
}

impl<'a> PirInput<'a> {
    /// Own the pin with a pull-down: the PIR idles low, and a
    /// disconnected front end reads as idle — not as permanent motion.
    pub(super) fn new(pin: esp_hal::peripherals::GPIO4<'static>) -> Self {
        Self {
            pin: Input::new(pin, InputConfig::default().with_pull(Pull::Down)),
            high: false,
        }
    }
}

impl EdgeSource for PirInput<'_> {
    async fn next_edge(&mut self) -> Edge {
        // Re-sync against the pad first: if the line moved while nobody
        // was waiting (the first arm, or a wait the driver left mid-
        // episode), report that transition instead of waiting for a
        // duplicate that will never come.
        let now = self.pin.is_high();
        if now != self.high {
            self.high = now;
            return if now { Edge::Rising } else { Edge::Falling };
        }
        // Arm the interrupt for the edge that flips the tracked level.
        if self.high {
            self.pin.wait_for_falling_edge().await;
        } else {
            self.pin.wait_for_rising_edge().await;
        }
        self.high = !self.high;
        if self.high {
            Edge::Rising
        } else {
            Edge::Falling
        }
    }
}
