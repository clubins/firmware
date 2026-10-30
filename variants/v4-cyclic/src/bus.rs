//! Bus plumbing — the shared peripheral handles drivers are handed.
//!
//! The module is always compiled (the [`Sensor`] trait takes `&mut Bus`), but
//! each backend only exists behind its plumbing flag, so a bus is exactly as
//! big as the drivers compiled in. Real peripheral types (esp-hal I2C/UART/
//! GPIO) land with the driver bring-up tasks; these are the static seams.

/// Shared bus handles handed to drivers at init and sample time.
pub struct Bus {
    #[cfg(feature = "driver-i2c")]
    pub i2c: I2c,
    #[cfg(feature = "driver-uart")]
    pub uart: Uart,
    #[cfg(feature = "driver-gpio")]
    pub gpio: Gpio,
}

impl Bus {
    /// A bus with no backends — driver stubs run against this.
    pub const fn stub() -> Self {
        Self {
            #[cfg(feature = "driver-i2c")]
            i2c: I2c,
            #[cfg(feature = "driver-uart")]
            uart: Uart,
            #[cfg(feature = "driver-gpio")]
            gpio: Gpio,
        }
    }
}

/// I2C controller handle — placeholder for the esp-hal peripheral.
#[cfg(feature = "driver-i2c")]
#[derive(Clone, Copy, Debug)]
pub struct I2c;

#[cfg(feature = "driver-i2c")]
impl I2c {
    pub const fn stub() -> Self {
        Self
    }
}

/// UART controller handle — placeholder for the esp-hal peripheral.
#[cfg(feature = "driver-uart")]
#[derive(Clone, Copy, Debug)]
pub struct Uart;

#[cfg(feature = "driver-uart")]
impl Uart {
    pub const fn stub() -> Self {
        Self
    }
}

/// GPIO bank handle — placeholder for the esp-hal peripheral.
#[cfg(feature = "driver-gpio")]
#[derive(Clone, Copy, Debug)]
pub struct Gpio;

#[cfg(feature = "driver-gpio")]
impl Gpio {
    pub const fn stub() -> Self {
        Self
    }
}
