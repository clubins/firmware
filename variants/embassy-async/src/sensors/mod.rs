//! Concrete sensor drivers — the flag-scoped hardware that runs through the
//! [`crate::sensor`] framework.
//!
//! One file per sensor family, each behind its own cargo feature
//! (`sensor-bme280`, `sensor-sht4x`, `sensor-scd4x`, …): a driver struct that
//! owns its bus handle and implements
//! [`SensorDriver`](crate::sensor::SensorDriver) (init → calibrate → sample),
//! converting wire bytes into the payload type its ingest topic speaks.
//!
//! Shared seams live here, in [`gpio`], [`i2c`], and [`uart`]:
//!
//! - [`Wait`] — the driver-level time seam, the sibling of the framework's
//!   `sensor::Sleep`. `Sleep` spaces the *task's* steps (the sampling
//!   cadence); `Wait` spaces the transactions *inside* one driver operation
//!   (a forced-mode conversion, a Sensirion measurement window, a data-ready
//!   poll). Like `Sleep` it is dependency inversion with one purpose: the
//!   `no_std` library never imports `embassy-time`, so the register flows
//!   run on the host against a recorded, deterministic wait.
//!
//! On the device, `device/tasks.rs` constructs each driver over its front
//! end — the shared I2C controller, the UART1 controller, the PIR's GPIO
//! pin — and hands it to the task body (the sampling framework's `run`, or
//! the PIR's dedicated event loop); on the host, the seams' `fakes`
//! modules script the bus transactions and the edge sequences.

#[cfg(feature = "sensor-bme280")]
pub mod bme280;
#[cfg(feature = "driver-gpio")]
pub mod gpio;
#[cfg(feature = "driver-i2c")]
pub mod i2c;
#[cfg(feature = "sensor-pax")]
pub mod pir;
#[cfg(feature = "sensor-pms5003")]
pub mod pms5003;
#[cfg(feature = "sensor-scd4x")]
pub mod scd4x;
#[cfg(any(feature = "sensor-sht4x", feature = "sensor-scd4x"))]
pub mod sensirion;
#[cfg(feature = "sensor-sht4x")]
pub mod sht4x;
#[cfg(feature = "driver-uart")]
pub mod uart;

/// Wait for `ms` milliseconds of hardware time (a conversion window, a
/// measurement period). Implementations yield the executor while they wait —
/// that is the point of the seam: a driver that waits is still an async task
/// other tasks can run alongside, on the device and in the executor-free
/// host tests.
///
/// Sleep vs wait, for the reader: `sensor::Sleep` orders a sensor task's
/// steps (cadence, backoff); [`Wait`] orders the microsecond-to-second
/// delays *within* one driver operation. A task holds both — the framework
/// clock and a driver wait — but never confuses them.
#[allow(async_fn_in_trait)]
pub trait Wait {
    async fn wait(&mut self, ms: u64);
}

// Every &mut W is also a waiter — the same variance rule `sensor::Sleep`
// follows, so drivers can borrow a shared waiter in tests.
impl<W: Wait + ?Sized> Wait for &mut W {
    async fn wait(&mut self, ms: u64) {
        (*self).wait(ms).await;
    }
}
