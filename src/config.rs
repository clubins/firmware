//! Runtime configuration — placeholder.
//!
//! The committed defaults live at [`DEFAULTS_PATH`]; the typed config model,
//! parsing, and the NVS-over-TOML-over-built-in precedence rule (spec §The
//! Config File) land with the configuration PR. Nothing reads this module
//! yet; it anchors the layout for the module-presence test.

/// Path of the committed default config, relative to the crate root.
pub const DEFAULTS_PATH: &str = "config/default.toml";

/// Module marker for the presence test.
pub const MODULE: &str = "config";
