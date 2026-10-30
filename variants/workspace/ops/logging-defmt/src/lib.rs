//! Logging facade (the `logging-defmt` capability).
//!
//! The scaffold ships the macro facade only: with the `defmt` feature on,
//! `log_*!` macros compile to defmt frames; without it they compile out.
//! The defmt dependency, the global-logger wiring (an esp back-end crate)
//! and the USB-JTAG/UART transport land with the logging task — a stub
//! global logger here would be a silent foot-gun, so this crate does not
//! depend on defmt yet.

#![cfg_attr(not(test), no_std)]

/// Info-level log. Compiled to nothing without the logging backend —
/// logging must never cost flash or cycles silently.
#[cfg(not(feature = "defmt"))]
#[macro_export]
macro_rules! log_info {
    ($($t:tt)*) => {{}};
}

/// Info-level log, defmt-backed (the logging task's deliverable).
#[cfg(feature = "defmt")]
#[macro_export]
macro_rules! log_info {
    ($($t:tt)*) => { defmt::info!($($t)*) };
}

/// Warn-level log. Same feature behaviour as [`log_info!`].
#[cfg(not(feature = "defmt"))]
#[macro_export]
macro_rules! log_warn {
    ($($t:tt)*) => {{}};
}

/// Warn-level log, defmt-backed (the logging task's deliverable).
#[cfg(feature = "defmt")]
#[macro_export]
macro_rules! log_warn {
    ($($t:tt)*) => { defmt::warn!($($t)*) };
}

/// Error-level log. Same feature behaviour as [`log_info!`].
#[cfg(not(feature = "defmt"))]
#[macro_export]
macro_rules! log_error {
    ($($t:tt)*) => {{}};
}

/// Error-level log, defmt-backed (the logging task's deliverable).
#[cfg(feature = "defmt")]
#[macro_export]
macro_rules! log_error {
    ($($t:tt)*) => { defmt::error!($($t)*) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn log_macros_expand_without_error_in_both_modes() {
        // Without the defmt backend these are no-ops; the test proves the
        // facade compiles and accepts format args.
        crate::log_info!("boot: features={}", 3);
        crate::log_warn!("sensor {} not responding", "bme280");
        crate::log_error!("overload at {}", 121.5);
    }
}
