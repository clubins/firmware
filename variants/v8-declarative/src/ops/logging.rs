//! defmt logging transport (`logging-defmt` feature).
//!
//! The UART/USB-JTAG defmt sink lands in the driver PR; the level filter
//! is fixed here because every subsystem shares it. Without the feature,
//! the rest of the crate compiles with logging compiled out entirely —
//! the flag's whole job.

/// Severity of a diagnostic line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// A line passes when its severity is at or above the configured maximum.
pub fn enabled(level: Level, max_level: Level) -> bool {
    level <= max_level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_admits_only_lines_at_or_above_the_maximum() {
        assert!(enabled(Level::Error, Level::Error));
        assert!(!enabled(Level::Warn, Level::Error));
        assert!(enabled(Level::Trace, Level::Trace));
        assert!(!enabled(Level::Trace, Level::Info));
    }
}
