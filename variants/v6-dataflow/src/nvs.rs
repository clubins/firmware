//! The NVS override layer — the runtime half of the config precedence.
//!
//! Spec §Configuration: NVS overrides sit on top of `config/default.toml`
//! (`built-in < file < NVS`) and carry the per-device material — sensor
//! identity, credentials, calibration, and any tuned tunable — so a device
//! is configured without a rebuild. Secrets exist **only** here: the
//! committed file holds placeholders, and [`SECRET_KEYS`] is the registry
//! of keys that must never appear in a `Config` or its rendered form.
//!
//! The layer is a source trait, not a concrete store. On the host, tests
//! and tools inject a [`MemorySource`]; on the ESP32-S3 the
//! [`NvsSource`] adapter (compiled only for the Xtensa target) exposes the
//! boot-time NVS snapshot the same way. [`crate::config::Config`] consults
//! the source key by key, so nothing enumerates NVS namespaces at runtime.
//!
//! Values are TOML scalars in text form — `"60"`, `"120.5"`, `"0x76"`,
//! `"\"http\""` — so one scanner ([`crate::config::parse_value`]) covers
//! both layers: string values carry quotes, numbers and hex do not.
//! Binary material (certificates, keys) is read by the transports through
//! [`OverrideSource::get`] into their own buffers, not through the config
//! path.

use crate::config::FixedStr;

/// The keys that hold secret material. They live only in NVS: no config
/// struct has a field for them, they are absent from
/// [`crate::config::KNOWN_PATHS`], and the committed `default.toml` holds
/// placeholders at most. The config layer's `debug_assert` tripwire fails
/// a debug build if a refactor ever routes one of these into a `Config`.
pub const SECRET_KEYS: [&str; 5] = [
    "wifi.ssid",
    "wifi.password",
    "http.apikey",
    "mqtt.client_cert",
    "mqtt.client_key",
];

/// The NVS namespace override keys live in (device partition layout:
/// `"clubins"` namespace, standard ESP-IDF NVS table).
pub const NVS_NAMESPACE: &str = "clubins";

/// Why reading an override failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrideError {
    /// The destination buffer cannot hold the value. Callers reading
    /// large binary material (certificates) size their buffers from the
    /// value's documented maximum.
    BufferTooSmall,
    /// The source is on the far side of hardware I/O that failed.
    ReadFailed,
}

/// Where override key/values come from at boot. One method, buffer-based:
/// the caller provides storage, the source copies into it — the same
/// no-heap contract as every stage in this crate.
pub trait OverrideSource {
    /// Copies the value for `key` into `buf`, returning the byte count.
    /// `Ok(None)` — the source has no override for this key (the normal
    /// case for most tunables on most devices).
    fn get(&self, key: &str, buf: &mut [u8]) -> Result<Option<usize>, OverrideError>;
}

// ---------------------------------------------------------------------------
// MemorySource — the host/test/tools source
// ---------------------------------------------------------------------------

/// Override entries one `MemorySource` holds.
pub const MAX_OVERRIDES: usize = 16;

/// Longest override key (certificates ride transport-side buffers).
const OVERRIDE_KEY_MAX: usize = crate::config::MAX_KEY_LEN;

/// Longest override value carried in-memory (`transport.http_endpoint`
/// is the longest tunable; secrets read here stay within it too —
/// longer binary material goes through the transport's own storage).
const OVERRIDE_VALUE_MAX: usize = crate::config::MAX_VALUE_LEN;

/// One injectable override binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Override {
    key: FixedStr<OVERRIDE_KEY_MAX>,
    value: FixedStr<OVERRIDE_VALUE_MAX>,
}

impl Override {
    /// A binding, or `None` when either part exceeds its capacity.
    pub fn new(key: &str, value: &str) -> Option<Self> {
        Some(Self {
            key: FixedStr::from_str(key)?,
            value: FixedStr::from_str(value)?,
        })
    }

    /// The key.
    pub fn key(&self) -> &str {
        self.key.as_str()
    }

    /// The value.
    pub fn value(&self) -> &str {
        self.value.as_str()
    }
}

/// A fixed table of overrides, injected per boot. This is the host
/// personality of the NVS layer: tests seed it with `insert`, and the
/// precedence tests drive it exactly like a provisioned device.
#[derive(Clone, Copy, Debug)]
pub struct MemorySource {
    entries: [Option<Override>; MAX_OVERRIDES],
    len: usize,
}

impl MemorySource {
    /// An empty source (const: usable in a `StaticCell`).
    pub const EMPTY: Self = Self {
        entries: [const { None }; MAX_OVERRIDES],
        len: 0,
    };

    /// An empty source.
    pub fn new() -> Self {
        Self::EMPTY
    }

    /// Adds a binding. Inserting over an existing key replaces it (the
    /// last write wins, mirroring an NVS set). Returns `false` when the
    /// table is full or a part exceeds its capacity.
    pub fn insert(&mut self, key: &str, value: &str) -> bool {
        let Some(entry) = Override::new(key, value) else {
            return false;
        };
        for slot in self.entries[..self.len].iter_mut() {
            if let Some(e) = slot
                && e.key == entry.key
            {
                *slot = Some(entry);
                return true;
            }
        }
        if self.len == MAX_OVERRIDES {
            return false;
        }
        self.entries[self.len] = Some(entry);
        self.len += 1;
        true
    }

    /// The bindings, in insertion order (replacements keep first place).
    pub fn iter(&self) -> impl Iterator<Item = &Override> {
        self.entries[..self.len].iter().flatten()
    }
}

impl Default for MemorySource {
    fn default() -> Self {
        Self::new()
    }
}

impl OverrideSource for MemorySource {
    fn get(&self, key: &str, buf: &mut [u8]) -> Result<Option<usize>, OverrideError> {
        let found = self
            .iter()
            .find(|e| e.key.as_str() == key)
            .map(|e| e.value.as_str());
        let Some(value) = found else {
            return Ok(None);
        };
        copy_out(value.as_bytes(), buf)
    }
}

// ---------------------------------------------------------------------------
// NvsSource — the device personality (Xtensa only)
// ---------------------------------------------------------------------------

/// The ESP32-S3 NVS-backed source: a snapshot of the `clubins` namespace
/// read from the NVS partition at boot, before any consumer runs. Compiled
/// only for the Xtensa target — host builds and tests never see device
/// storage code, per the variant's two-personality rule.
///
/// The partition scan itself (flash read + key/value walk) lands with the
/// provisioning/storage PR (docs/plan/build-dag.md); until then the
/// snapshot starts empty, which resolves to the file/built-in layers —
/// the correct no-provisioning behavior. The [`OverrideSource`] seam this
/// type implements is what the scan fills.
#[cfg(target_arch = "xtensa")]
pub struct NvsSource {
    entries: [Option<Override>; MAX_OVERRIDES],
    len: usize,
}

#[cfg(target_arch = "xtensa")]
impl NvsSource {
    /// An empty snapshot, to be filled by the boot-time partition scan.
    pub const EMPTY: Self = Self {
        entries: [const { None }; MAX_OVERRIDES],
        len: 0,
    };

    /// An empty snapshot.
    pub fn new() -> Self {
        Self::EMPTY
    }

    /// Records one entry from the partition scan (provisioning PR's
    /// entry point). Returns `false` when the snapshot is full.
    pub fn record(&mut self, entry: Override) -> bool {
        if self.len == MAX_OVERRIDES {
            return false;
        }
        self.entries[self.len] = Some(entry);
        self.len += 1;
        true
    }
}

#[cfg(target_arch = "xtensa")]
impl Default for NvsSource {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "xtensa")]
impl OverrideSource for NvsSource {
    fn get(&self, key: &str, buf: &mut [u8]) -> Result<Option<usize>, OverrideError> {
        let found = self.entries[..self.len]
            .iter()
            .flatten()
            .find(|e| e.key.as_str() == key)
            .map(|e| e.value.as_str());
        let Some(value) = found else {
            return Ok(None);
        };
        copy_out(value.as_bytes(), buf)
    }
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

fn copy_out(value: &[u8], buf: &mut [u8]) -> Result<Option<usize>, OverrideError> {
    if value.len() > buf.len() {
        return Err(OverrideError::BufferTooSmall);
    }
    buf[..value.len()].copy_from_slice(value);
    Ok(Some(value.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_source_stores_and_returns_bindings() {
        let mut src = MemorySource::new();
        assert!(src.insert("sampling.report_interval_s", "30"));
        let mut buf = [0u8; 16];
        let n = src
            .get("sampling.report_interval_s", &mut buf)
            .expect("read")
            .expect("present");
        assert_eq!(&buf[..n], b"30");
        assert_eq!(
            src.get("sampling.report_interval_s", &mut [0u8; 1]),
            Err(OverrideError::BufferTooSmall)
        );
        assert_eq!(src.get("sampling.offline_buffer", &mut buf), Ok(None));
    }

    #[test]
    fn reinserting_a_key_replaces_it() {
        let mut src = MemorySource::new();
        assert!(src.insert("sampling.report_interval_s", "30"));
        assert!(src.insert("sampling.report_interval_s", "45"));
        assert_eq!(src.iter().count(), 1);
        let mut buf = [0u8; 16];
        let n = src
            .get("sampling.report_interval_s", &mut buf)
            .expect("read")
            .expect("present");
        assert_eq!(&buf[..n], b"45");
    }

    #[test]
    fn empty_source_has_no_overrides() {
        let src = MemorySource::EMPTY;
        let mut buf = [0u8; 16];
        assert_eq!(src.get("sensor.id", &mut buf), Ok(None));
    }

    #[test]
    fn secret_keys_are_registry_complete() {
        // Every key a transport could need credentials for is in the
        // registry — the list the config layer's tripwire checks against.
        for key in SECRET_KEYS {
            assert!(!crate::config::KNOWN_PATHS.contains(&key));
        }
    }
}
