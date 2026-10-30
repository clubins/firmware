//! BLE Wi-Fi provisioning state (`provisioning` feature).
//!
//! NimBLE bring-up lands in the driver PR; the visibility rule is fixed
//! here because the transport loop and the status heartbeat both depend on
//! it: a device with credentials never advertises.

/// How long the BLE stack advertises before yielding (ms).
pub const ADV_WINDOW_MS: u32 = 500;

/// A device advertises only while it has no Wi-Fi credentials in NVS.
pub fn advertising_window_ms(has_credentials: bool) -> u32 {
    if has_credentials { 0 } else { ADV_WINDOW_MS }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioned_devices_never_advertise() {
        assert_eq!(advertising_window_ms(true), 0);
        assert_eq!(advertising_window_ms(false), ADV_WINDOW_MS);
    }
}
