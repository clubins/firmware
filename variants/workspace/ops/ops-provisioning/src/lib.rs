//! BLE-based Wi-Fi provisioning (the `provisioning` capability).
//!
//! The BLE peripheral and NVS write land with the provisioning task; this
//! scaffold pins the request model and its state machine so transport code
//! can already integrate against them.

#![cfg_attr(not(test), no_std)]

/// One provisioning attempt, from advertisement to stored credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningState {
    /// Advertising, waiting for the companion app.
    Advertising,
    /// Credentials received, not yet stored.
    Receiving,
    /// Credentials validated and written to NVS; the device reboots into
    /// station mode.
    Stored,
    /// The app cancelled or the exchange failed; still advertising.
    Failed,
}

/// What the companion app sends over BLE. Parsed from the characteristic
/// payload by the provisioning task; the NVS write happens only on
/// transition to `Stored`.
// Debug is hand-rolled below so the psk never renders.
#[derive(Clone, PartialEq, Eq)]
pub struct ProvisioningRequest {
    ssid: heapless::String<32>,
    psk: heapless::String<64>,
}

/// The request payload was malformed (missing delimiter, oversized field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MalformedRequest;

impl ProvisioningRequest {
    /// Payload format: `<ssid>\n<psk>` — the BLE task hands us the split
    /// fields; this constructor enforces the bounds.
    pub fn new(ssid: &str, psk: &str) -> Result<Self, MalformedRequest> {
        if ssid.is_empty() || ssid.len() > 32 || psk.len() > 64 {
            return Err(MalformedRequest);
        }
        Ok(Self {
            ssid: heapless::String::try_from(ssid).map_err(|_| MalformedRequest)?,
            psk: heapless::String::try_from(psk).map_err(|_| MalformedRequest)?,
        })
    }

    pub fn ssid(&self) -> &str {
        &self.ssid
    }

    /// For the NVS write only; never for logging.
    pub fn psk(&self) -> &str {
        &self.psk
    }
}

impl core::fmt::Debug for ProvisioningRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProvisioningRequest")
            .field("ssid", &self.ssid.as_str())
            .field("psk", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_contains_the_psk() {
        let request = ProvisioningRequest::new("clubins-venue", "correct-horse").unwrap();
        let rendered = format!("{request:?}");
        assert!(rendered.contains("clubins-venue"));
        assert!(!rendered.contains("correct-horse"));
    }

    #[test]
    fn rejects_empty_ssid_and_oversized_fields() {
        assert_eq!(ProvisioningRequest::new("", "x"), Err(MalformedRequest));
        let long_ssid = "0123456789012345678901234567890123"; // 34 > 32
        assert_eq!(
            ProvisioningRequest::new(long_ssid, "x"),
            Err(MalformedRequest)
        );
        let long_psk =
            "012345678901234567890123456789012345678901234567890123456789012345"; // 66 > 64
        assert_eq!(
            ProvisioningRequest::new("venue", long_psk),
            Err(MalformedRequest)
        );
    }

    #[test]
    fn round_trips_the_ssid_for_the_nvs_write() {
        let request = ProvisioningRequest::new("clubins-venue", "correct-horse").unwrap();
        assert_eq!(request.ssid(), "clubins-venue");
        assert_eq!(request.psk(), "correct-horse");
    }
}
