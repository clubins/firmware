//! Over-the-air updates (the `ota` capability) on the esp-ota partition
//! scheme.
//!
//! The download, verify and switch logic lands with the OTA task; this
//! scaffold pins the state machine's states so the transport and ops code
//! can already name them.

#![cfg_attr(not(test), no_std)]

/// One OTA attempt's lifecycle. `Rollback` is what the bootloader does when
/// the new image never marks itself healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaState {
    Idle,
    Downloading,
    /// New image staged in the OTA slot, not yet confirmed.
    PendingVerify,
    /// New image confirmed good; subsequent resets use it.
    Activated,
    /// New image failed verification; the previous image stays active.
    Rollback,
}

/// Tracks an update from staging to activation. The real OTA task replaces
/// the state transitions with flash and bootloader operations.
#[derive(Debug)]
pub struct OtaUpdate {
    state: OtaState,
}

impl OtaUpdate {
    pub fn new() -> Self {
        Self {
            state: OtaState::Idle,
        }
    }

    pub fn state(&self) -> OtaState {
        self.state
    }

    pub fn begin_download(&mut self) {
        self.state = OtaState::Downloading;
    }

    pub fn mark_staged(&mut self) {
        self.state = OtaState::PendingVerify;
    }

    pub fn confirm(&mut self) {
        self.state = OtaState::Activated;
    }

    pub fn reject(&mut self) {
        self.state = OtaState::Rollback;
    }
}

impl Default for OtaUpdate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_happy_update_walks_the_whole_lifecycle() {
        let mut update = OtaUpdate::new();
        assert_eq!(update.state(), OtaState::Idle);
        update.begin_download();
        update.mark_staged();
        update.confirm();
        assert_eq!(update.state(), OtaState::Activated);
    }

    #[test]
    fn a_rejected_update_rolls_back() {
        let mut update = OtaUpdate::new();
        update.begin_download();
        update.mark_staged();
        update.reject();
        assert_eq!(update.state(), OtaState::Rollback);
    }
}
