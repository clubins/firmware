//! OTA update state machine (`ota` feature).
//!
//! The esp-ota partition scheme brings up in the driver PR; the state
//! transitions are fixed here because both the transport and the ops loop
//! reason about them.

/// Lifecycle of one OTA attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// No update pending.
    Idle,
    /// Image streaming into the OTA partition.
    Downloading,
    /// Image checksum-verified, ready to switch.
    Verified,
    /// Reboot into the verified image.
    Rebooting,
}

/// Only a verified image may trigger a reboot — never a half-downloaded one.
pub fn can_reboot(state: UpdateState) -> bool {
    matches!(state, UpdateState::Verified | UpdateState::Rebooting)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_verified_images_may_reboot() {
        assert!(!can_reboot(UpdateState::Idle));
        assert!(!can_reboot(UpdateState::Downloading));
        assert!(can_reboot(UpdateState::Verified));
        assert!(can_reboot(UpdateState::Rebooting));
    }
}
