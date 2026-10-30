//! BLE provisioning stub — gated by the `provisioning` flag.
//!
//! Wi-Fi credentials come from NVS only (never the committed config). BLE
//! bring-up and credential chunking arrive with the provisioning PR; the
//! stub tracks sessions.

#[derive(Debug)]
pub struct ProvisioningStub {
    /// Provisioning sessions opened since boot.
    pub sessions: u32,
}

impl ProvisioningStub {
    pub const fn new() -> Self {
        ProvisioningStub { sessions: 0 }
    }

    /// Opens a provisioning session (BLE advertise → credential exchange →
    /// NVS write). Returns the running session count.
    pub fn start_session(&mut self) -> u32 {
        self.sessions += 1;
        self.sessions
    }
}

impl Default for ProvisioningStub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_count_up() {
        let mut p = ProvisioningStub::new();
        assert_eq!(p.start_session(), 1);
        assert_eq!(p.start_session(), 2);
    }
}
