//! mTLS transport stub — gated by the `transport-mtls` flag (implies
//! `transport-mqtt`).
//!
//! Contract (api/mqtt/topics.md + server-infra/pki/): the client
//! certificate comes from NVS at runtime, CN = sensor UUID. The scaffold
//! has no NVS access yet — identity is unset until the config PR lands,
//! and this stub reports that honestly.

#[derive(Debug)]
pub struct MtlsStub {
    /// True once a client certificate + key are loaded from NVS. Always
    /// false in the scaffold (no NVS access yet).
    pub identity_configured: bool,
}

impl MtlsStub {
    pub const fn new() -> Self {
        MtlsStub {
            identity_configured: false,
        }
    }
}

impl Default for MtlsStub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_has_no_identity_yet() {
        let m = MtlsStub::new();
        assert!(!m.identity_configured);
    }
}
