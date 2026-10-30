//! The static sensor registry — built at boot from compiled drivers × runtime
//! enables.
//!
//! Every compiled driver consults config; a driver disabled in config is
//! absent from the registry, so nothing samples or publishes for it. Storage
//! is a fixed slot array with static dispatch (no vtables in the hot path) —
//! the time-triggered variant allocates nothing at run time.

use crate::bus::Bus;
#[cfg(feature = "any-sensor")]
use crate::sensor::Sensor;
use crate::sensor::{Sample, SensorError, SensorMeta};

pub const MAX_SENSORS: usize = 8;

/// Runtime half of the flag framework: which compiled drivers are enabled.
/// Fields exist only for compiled drivers — an uncompiled driver cannot be
/// enabled at run time, which is the compile/runtime split by design.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorsEnabled {
    #[cfg(feature = "sensor-decibel")]
    pub decibel: bool,
    #[cfg(feature = "sensor-pax")]
    pub pir: bool,
    #[cfg(feature = "sensor-bme280")]
    pub bme280: bool,
    #[cfg(feature = "sensor-sht4x")]
    pub sht4x: bool,
    #[cfg(feature = "sensor-scd4x")]
    pub scd4x: bool,
    #[cfg(feature = "sensor-pms5003")]
    pub pms5003: bool,
    #[cfg(feature = "sensor-bh1750")]
    pub bh1750: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// More than [`MAX_SENSORS`] drivers.
    Full,
    /// The driver failed to initialize on its bus.
    InitFailed(SensorError),
}

/// Every compiled-in driver, statically dispatched.
pub enum SensorSlot {
    #[cfg(feature = "sensor-decibel")]
    Decibel(crate::sensors::decibel::Decibel),
    #[cfg(feature = "sensor-pax")]
    Pax(crate::sensors::pir::Pir),
    #[cfg(feature = "sensor-bme280")]
    Bme280(crate::sensors::bme280::Bme280),
    #[cfg(feature = "sensor-sht4x")]
    Sht4x(crate::sensors::sht4x::Sht4x),
    #[cfg(feature = "sensor-scd4x")]
    Scd4x(crate::sensors::scd4x::Scd4x),
    #[cfg(feature = "sensor-pms5003")]
    Pms5003(crate::sensors::pms5003::Pms5003),
    #[cfg(feature = "sensor-bh1750")]
    Bh1750(crate::sensors::bh1750::Bh1750),
}

impl SensorSlot {
    pub fn meta(&self) -> &'static SensorMeta {
        match *self {
            #[cfg(feature = "sensor-decibel")]
            Self::Decibel(ref s) => s.meta(),
            #[cfg(feature = "sensor-pax")]
            Self::Pax(ref s) => s.meta(),
            #[cfg(feature = "sensor-bme280")]
            Self::Bme280(ref s) => s.meta(),
            #[cfg(feature = "sensor-sht4x")]
            Self::Sht4x(ref s) => s.meta(),
            #[cfg(feature = "sensor-scd4x")]
            Self::Scd4x(ref s) => s.meta(),
            #[cfg(feature = "sensor-pms5003")]
            Self::Pms5003(ref s) => s.meta(),
            #[cfg(feature = "sensor-bh1750")]
            Self::Bh1750(ref s) => s.meta(),
        }
    }

    /// Initialize the wrapped driver — the push path runs init once.
    #[cfg_attr(not(feature = "any-sensor"), allow(unused_variables))]
    pub fn init(&mut self, bus: &mut Bus) -> Result<(), SensorError> {
        match *self {
            #[cfg(feature = "sensor-decibel")]
            Self::Decibel(ref mut s) => s.init(bus),
            #[cfg(feature = "sensor-pax")]
            Self::Pax(ref mut s) => s.init(bus),
            #[cfg(feature = "sensor-bme280")]
            Self::Bme280(ref mut s) => s.init(bus),
            #[cfg(feature = "sensor-sht4x")]
            Self::Sht4x(ref mut s) => s.init(bus),
            #[cfg(feature = "sensor-scd4x")]
            Self::Scd4x(ref mut s) => s.init(bus),
            #[cfg(feature = "sensor-pms5003")]
            Self::Pms5003(ref mut s) => s.init(bus),
            #[cfg(feature = "sensor-bh1750")]
            Self::Bh1750(ref mut s) => s.init(bus),
        }
    }

    #[cfg_attr(not(feature = "any-sensor"), allow(unused_variables))]
    pub fn sample(&mut self, bus: &mut Bus) -> Result<Option<Sample>, SensorError> {
        match *self {
            #[cfg(feature = "sensor-decibel")]
            Self::Decibel(ref mut s) => s.sample(bus),
            #[cfg(feature = "sensor-pax")]
            Self::Pax(ref mut s) => s.sample(bus),
            #[cfg(feature = "sensor-bme280")]
            Self::Bme280(ref mut s) => s.sample(bus),
            #[cfg(feature = "sensor-sht4x")]
            Self::Sht4x(ref mut s) => s.sample(bus),
            #[cfg(feature = "sensor-scd4x")]
            Self::Scd4x(ref mut s) => s.sample(bus),
            #[cfg(feature = "sensor-pms5003")]
            Self::Pms5003(ref mut s) => s.sample(bus),
            #[cfg(feature = "sensor-bh1750")]
            Self::Bh1750(ref mut s) => s.sample(bus),
        }
    }
}

pub struct Registry {
    slots: [Option<SensorSlot>; MAX_SENSORS],
    len: usize,
}

impl Registry {
    /// Empty registry — `no-default-features` builds run this for real.
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; MAX_SENSORS],
            len: 0,
        }
    }

    /// Build the registry: every compiled driver consults `enabled`; a driver
    /// disabled there is absent (never sampled, never published).
    #[cfg_attr(not(feature = "any-sensor"), allow(unused_mut, unused_variables))]
    pub fn from_config(enabled: &SensorsEnabled, bus: &mut Bus) -> Result<Self, RegistryError> {
        let mut registry = Self::new();
        #[cfg(feature = "sensor-decibel")]
        if enabled.decibel {
            registry.push(
                SensorSlot::Decibel(crate::sensors::decibel::Decibel::new()),
                bus,
            )?;
        }
        #[cfg(feature = "sensor-pax")]
        if enabled.pir {
            registry.push(SensorSlot::Pax(crate::sensors::pir::Pir::new()), bus)?;
        }
        #[cfg(feature = "sensor-bme280")]
        if enabled.bme280 {
            registry.push(
                SensorSlot::Bme280(crate::sensors::bme280::Bme280::new()),
                bus,
            )?;
        }
        #[cfg(feature = "sensor-sht4x")]
        if enabled.sht4x {
            registry.push(SensorSlot::Sht4x(crate::sensors::sht4x::Sht4x::new()), bus)?;
        }
        #[cfg(feature = "sensor-scd4x")]
        if enabled.scd4x {
            registry.push(SensorSlot::Scd4x(crate::sensors::scd4x::Scd4x::new()), bus)?;
        }
        #[cfg(feature = "sensor-pms5003")]
        if enabled.pms5003 {
            registry.push(
                SensorSlot::Pms5003(crate::sensors::pms5003::Pms5003::new()),
                bus,
            )?;
        }
        #[cfg(feature = "sensor-bh1750")]
        if enabled.bh1750 {
            registry.push(
                SensorSlot::Bh1750(crate::sensors::bh1750::Bh1750::new()),
                bus,
            )?;
        }
        Ok(registry)
    }

    #[cfg(feature = "any-sensor")]
    fn push(&mut self, mut slot: SensorSlot, bus: &mut Bus) -> Result<(), RegistryError> {
        if self.len == MAX_SENSORS {
            return Err(RegistryError::Full);
        }
        slot.init(bus).map_err(RegistryError::InitFailed)?;
        self.slots[self.len] = Some(slot);
        self.len += 1;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Metadata for the driver at `slot`, in registry order.
    pub fn meta(&self, slot: usize) -> &'static SensorMeta {
        self.slots[slot]
            .as_ref()
            .expect("slots below len are filled")
            .meta()
    }

    /// Poll the driver at `slot` — the sensor phase's inner loop.
    pub fn sample(&mut self, slot: usize, bus: &mut Bus) -> Result<Option<Sample>, SensorError> {
        self.slots[slot]
            .as_mut()
            .expect("slots below len are filled")
            .sample(bus)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
