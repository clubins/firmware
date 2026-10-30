//! defmt logging stub — gated by the `logging-defmt` flag.
//!
//! The defmt transport over UART/USB-JTAG arrives with the logging PR; the
//! stub pins the level-gating policy. Secret redaction is a standing rule:
//! credentials, API keys, and client keys are never logged — only their
//! presence/absence (spec §Config; hard rule in docs/plan/build-dag.md).

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
}

/// True when `msg` is severe enough to pass a `min` filter.
pub const fn should_log(min: Level, msg: Level) -> bool {
    (msg as u8) <= (min as u8)
}

#[derive(Debug)]
pub struct LoggingStub {
    /// Minimum level the compiled transport emits.
    pub min: Level,
}

impl LoggingStub {
    pub const fn new() -> Self {
        LoggingStub { min: Level::Info }
    }

    /// Level gate for one message: would this level be emitted?
    pub fn log(&mut self, msg: Level) -> bool {
        should_log(self.min, msg)
    }
}

impl Default for LoggingStub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_gate_is_total_ordering() {
        assert!(should_log(Level::Error, Level::Error));
        assert!(should_log(Level::Info, Level::Error));
        assert!(!should_log(Level::Info, Level::Debug));
        assert!(should_log(Level::Debug, Level::Warn));
    }

    #[test]
    fn stub_defaults_to_info_gate() {
        let mut l = LoggingStub::new();
        assert!(l.log(Level::Warn));
        assert!(!l.log(Level::Debug));
    }
}
