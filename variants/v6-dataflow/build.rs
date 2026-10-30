//! Build support for the esp-hal device build.
//!
//! Linker setup for the ESP32-S3 lives in `.cargo/config.toml` (rust-lld +
//! `-Tlinkall.x`, per docs/research/firmware-internals-build-and-ci.md);
//! esp-hal contributes its chip linker scripts through that flag. Keep this
//! script minimal — it runs on every rebuild, host and target alike.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
