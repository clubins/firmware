//! Clubins firmware — V7 dual-core partitioned variant.
//!
//! One crate, two personalities (spec §Architecture): the pure-logic modules
//! here — the cross-core message model ([`boundary`]) and the partition
//! stubs ([`core0`], [`core1`]) — compile and test on the host; everything
//! hardware-bound lives in the target-only bin (`main.rs`) and in esp-hal,
//! declared as a dependency for the Xtensa target only.
//!
//! Architecture (V7, strict core partitioning): core 1 (the APP CPU) owns
//! sensing + DSP on a deterministic real-time loop; core 0 (the PRO CPU)
//! owns Wi-Fi, the transports, and ops on a latency-tolerant loop. The
//! cores never share state — every crossing goes through [`boundary`]: a
//! bounded SPSC queue of measurements (core 1 → core 0), a tiny SPSC ack
//! queue (core 1 → core 0), and the control queue whose producer-side
//! discipline ([`mailbox`]) makes it a latest-wins mailbox (core 0 →
//! core 1). A network-side stall can therefore never block sampling; at
//! worst the measurement queue back-pressures and core 1 counts drops.
//!
//! Each of the 15 public feature flags (spec §Feature Matrix) compiles one
//! stub into a partition — sensors into core 1, transports + ops into core
//! 0 — and any combination must compile (`just flag-matrix` verifies
//! all-off, each flag alone, all-on).

#![cfg_attr(not(test), no_std)]
// unsafe is denied crate-wide, with exactly one allow: the app-core stack
// handoff in cores.rs::boot, which esp-hal's start_app_core signature
// requires (&'static mut from a static). Every queue and mailbox primitive
// is unsafe-free by design — heapless's split halves do the atomics.
#![deny(unsafe_code)]

extern crate alloc;

pub mod boundary;

pub mod core0;
pub mod core1;
pub mod cores;
pub mod mailbox;

// The device entry point — Xtensa-only (cores::boot is cfg'd the same
// way); main.rs's device personality calls it at the crate root.
#[cfg(target_arch = "xtensa")]
pub use cores::boot;
