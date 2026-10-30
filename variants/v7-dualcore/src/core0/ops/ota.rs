//! OTA update stub — gated by the `ota` flag.
//!
//! esp-ota partition scheme (verify/reboot) arrives with the OTA PR; the
//! stub pins the lifecycle states the real implementation will walk.

/// Lifecycle of an update attempt. Lives on core 0 (the ops core);
/// a reboot request drains the sensing core through the control mailbox
/// (`Control::Shutdown`) before the partition switchover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaState {
    /// No update in progress.
    Idle,
    /// Manifest fetched, comparing versions.
    Checking,
    /// Image downloading to the OTA partition.
    Downloading,
    /// Image hash verified, reboot pending.
    RebootPending,
}

#[derive(Debug)]
pub struct OtaStub {
    pub state: OtaState,
}

impl OtaStub {
    pub const fn new() -> Self {
        OtaStub {
            state: OtaState::Idle,
        }
    }

    /// One ops tick of the update machine. No update server exists in the
    /// scaffold, so the machine peeks at the manifest and returns to idle:
    /// Idle → Checking → Idle.
    pub fn tick(&mut self) -> OtaState {
        self.state = match self.state {
            OtaState::Idle => OtaState::Checking,
            OtaState::Checking => OtaState::Idle,
            state => state,
        };
        self.state
    }
}

impl Default for OtaStub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_machine_peeks_and_returns_to_idle() {
        let mut o = OtaStub::new();
        assert_eq!(o.state, OtaState::Idle);
        assert_eq!(o.tick(), OtaState::Checking);
        assert_eq!(o.tick(), OtaState::Idle);
    }
}
