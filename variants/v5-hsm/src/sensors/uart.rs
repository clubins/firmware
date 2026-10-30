//! The sensors' UART transport abstraction — the byte-stream voice of the
//! Plantower-family sensors. The I2C counterpart lives in
//! [`crate::sensors::bus`]; the same closed-error and never-block rules
//! apply.

/// A closed error set for UART transactions. Target adapters map their
/// driver-specific errors into this set; drivers match on nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartError {
    /// The receive side dropped bytes before the driver drained them.
    Overrun,
    /// Any other transport-level failure (framing noise, parity, ...).
    Other,
}

/// Synchronous UART transactions — the byte-stream voice of the drivers.
/// Mirrors the embedded-hal serial operation set (write / read) so the
/// future `esp-hal` adapter is a thin wrapper. Reads never block: they
/// hand back whatever has arrived, up to `buf.len()`.
pub trait UartIo {
    /// Writes command bytes to the sensor.
    fn write(&mut self, bytes: &[u8]) -> Result<(), UartError>;
    /// Drains up to `buf.len()` received bytes; returns how many landed.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, UartError>;
}
