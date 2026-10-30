//! Clubins V4 firmware — time-triggered cyclic executive.
//!
//! A deterministic tick scheduler replaces any superloop: a hardware timer
//! interrupt advances a fixed phase table (sensor → processing → comms →
//! ops), ISRs defer work into lock-free queues consumed in their phase, and a
//! bounded timer wheel handles slow cadences. Bounded jitter and phase
//! ordering are first-class guarantees, checked by host tests.
//!
//! # Architecture map
//!
//! ```text
//!  ISR context (hardware timer, GPIO edges, UART RX)
//!    │  push events into SpscRing — never run work inline
//!    ▼
//!  Executive::advance ──── PhaseTable (fixed order, periodic slots)
//!    │   ├─ sensor phase     Registry::sample → Sample
//!    │   ├─ processing phase DSP / aggregation (decibel windows)
//!    │   ├─ comms phase      Transport::publish / poll
//!    │   └─ ops phase        status heartbeats, watchdog service
//!    ▼
//!  TimerWheel ── slow cadences (>1 tick periods) as scheduled callbacks
//!  JitterTracker ── per-slot max/avg lateness, surfaced in BatchReport
//! ```
//!
//! # Flag framework
//!
//! Fifteen public leaf flags (see README for the full matrix): seven sensor
//! drivers, two transports (+mTLS), and four ops capabilities, plus internal
//! plumbing flags keeping dependency graphs honest. Any combination must
//! compile — CI builds all-off, each flag alone, and all-on. Runtime tunables
//! come from committed `config/default.toml` with NVS overrides (later task);
//! secrets live only in NVS.

#![no_std]

pub mod bus;
#[cfg(feature = "sensor-decibel")]
pub mod dsp;
pub mod measurement;
#[cfg(feature = "net-wifi")]
pub mod net;
pub mod ops;
#[cfg(feature = "proto-v2")]
pub mod proto;
pub mod registry;
pub mod sensor;
pub mod sensors;
pub mod transport;

#[cfg(feature = "driver-gpio")]
pub use bus::Gpio;
#[cfg(feature = "driver-i2c")]
pub use bus::I2c;
#[cfg(feature = "driver-uart")]
pub use bus::Uart;
