//! Wi-Fi STA bring-up (the internal `net-wifi` capability).
//!
//! The esp-hal Wi-Fi stack connection, SNTP and the offline buffer drain land
//! with the transport-core task; this scaffold pins the credential type and
//! its secret hygiene. Credentials are read from NVS at runtime — never from
//! the committed config, never logged.

#![cfg_attr(not(test), no_std)]

use heapless::String;

/// An error for credentials that do not fit the fixed-size buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialsTooLong;

/// Wi-Fi station credentials with a redacting Debug: an accidental
/// `{:?}` log of a struct holding this must not print the key.
pub struct WifiCredentials {
    ssid: String<32>,
    psk: String<64>,
}

impl WifiCredentials {
    pub fn new(ssid: &str, psk: &str) -> Result<Self, CredentialsTooLong> {
        Ok(Self {
            ssid: String::try_from(ssid).map_err(|_| CredentialsTooLong)?,
            psk: String::try_from(psk).map_err(|_| CredentialsTooLong)?,
        })
    }

    pub fn ssid(&self) -> &str {
        &self.ssid
    }

    /// Credential accessor for the Wi-Fi stack bring-up only; never for
    /// logging.
    pub fn psk(&self) -> &str {
        &self.psk
    }
}

impl core::fmt::Debug for WifiCredentials {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WifiCredentials")
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
        let credentials =
            WifiCredentials::new("clubins-venue", "correct-horse-battery").unwrap();
        let rendered = format!("{credentials:?}");
        assert!(rendered.contains("clubins-venue"));
        assert!(!rendered.contains("correct-horse-battery"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn rejects_oversized_ssid_and_psk() {
        let long_ssid = "0123456789012345678901234567890123"; // 34 > 32
        assert!(matches!(
            WifiCredentials::new(long_ssid, "x"),
            Err(CredentialsTooLong)
        ));
        let long_psk =
            "012345678901234567890123456789012345678901234567890123456789012345"; // 66 > 64
        assert!(matches!(
            WifiCredentials::new("venue", long_psk),
            Err(CredentialsTooLong)
        ));
    }

    #[test]
    fn round_trips_the_ssid() {
        let credentials = WifiCredentials::new("clubins-venue", "secret").unwrap();
        assert_eq!(credentials.ssid(), "clubins-venue");
        assert_eq!(credentials.psk(), "secret");
    }
}
