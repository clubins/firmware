//! HTTP JSON ingest transport (the `transport-http` capability) — the only
//! path that reaches ClickHouse today.
//!
//! The JSON payload encoder and the POST/backoff/offline-buffer machinery
//! land with the HTTP transport task, golden-tested field-for-field against
//! the Go ingest structs (`server-infra/ingest/service/*.go`); this scaffold
//! pins the URL contract and the API-key hygiene.

#![cfg_attr(not(test), no_std)]

use core::fmt::Write;
use heapless::String;

/// The endpoint buffer is large enough for any sane ingest host.
pub const URL_BUFFER: usize = 160;

/// The URL for one ingest POST: `<base>/<type>`. With the spec's default
/// endpoint this is e.g. `https://ingest.clubins.de/v1/sensors/decibel`.
/// Caddy strips `/v1` before the Go service, which mounts `/sensors/<type>`.
pub fn ingest_url(base: &str, measurement_type: &str) -> String<URL_BUFFER> {
    let mut url = String::<URL_BUFFER>::new();
    core::write!(url, "{base}/{measurement_type}").expect("ingest url buffer");
    url
}

/// The per-sensor API key sent in every body (`apikey`).
///
/// Loaded from NVS at runtime — never committed, never logged. The Debug
/// impl redacts, so an accidental `{:?}` cannot leak the key.
pub struct ApiKey(String<64>);

/// An error for an API key that does not fit the fixed-size buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiKeyTooLong;

impl ApiKey {
    pub fn new(key: &str) -> Result<Self, ApiKeyTooLong> {
        String::try_from(key).map(Self).map_err(|_| ApiKeyTooLong)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("ApiKey").field(&"<redacted>").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_urls_match_the_server_endpoints() {
        let base = "https://ingest.clubins.de/v1/sensors";
        for measurement_type in ["decibel", "pax", "weather", "status"] {
            let url = ingest_url(base, measurement_type);
            assert_eq!(
                url.as_str(),
                format!("{base}/{measurement_type}"),
                "endpoint for {measurement_type}"
            );
        }
    }

    #[test]
    fn api_key_debug_output_is_redacted() {
        let key = ApiKey::new("sk-live-0123456789").unwrap();
        let rendered = format!("{key:?}");

        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("sk-live-0123456789"));
    }

    #[test]
    fn api_key_round_trips_for_the_post_body() {
        let key = ApiKey::new("sk-live-0123456789").unwrap();
        assert_eq!(key.as_str(), "sk-live-0123456789");
    }
}
