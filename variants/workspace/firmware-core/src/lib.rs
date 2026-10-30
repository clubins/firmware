//! Pure firmware core: the measurement model, the sensor contract, the
//! sensor registry, and the runtime config shape.
//!
//! Everything here is no_std and host-testable; hardware glue lives in the
//! driver, transport and ops crates and is compile-verified on the Xtensa
//! target.

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod config;
pub mod measurement;
pub mod registry;
pub mod sensor;
