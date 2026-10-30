//! Core 1 partition — sensing + DSP (deterministic real-time).
//!
//! Charter (spec §Architecture): this core never touches Wi-Fi, sockets, or
//! the heap in steady state; its loop is apply-control → poll-drivers in a
//! fixed order, with static dispatch (named driver fields, no vtables) so
//! worst-case timing stays inspectable. The composition root
//! (`Core1System`) and the runtimes that step it land with the cross-core
//! primitive (crate::cores); the driver contract is here.

pub mod sensors {
    //! Sensor driver stubs — one module per `sensor-*` flag, all owned by
    //! the core-1 sampling partition. Real drivers ship in downstream PRs
    //! (docs/plan/build-dag.md); the stubs pin the [`super::SensorDriver`]
    //! signature they must fill in.

    #[cfg(feature = "sensor-bh1750")]
    pub mod bh1750;
    #[cfg(feature = "sensor-bme280")]
    pub mod bme280;
    #[cfg(feature = "sensor-decibel")]
    pub mod decibel;
    #[cfg(feature = "sensor-pax")]
    pub mod pax;
    #[cfg(feature = "sensor-pms5003")]
    pub mod pms5003;
    #[cfg(feature = "sensor-scd4x")]
    pub mod scd4x;
    #[cfg(feature = "sensor-sht4x")]
    pub mod sht4x;
}

use crate::boundary::{Ack, Control, Core1Link, Measurement, SamplingPartition};

/// Static descriptor for one driver: its name and the ingest type it feeds
/// (one-topic-per-type rule — spec §Sensor Expansion).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorMeta {
    pub name: &'static str,
    pub ingest_type: &'static str,
}

/// Anything that can sit on the core-1 sampling loop.
pub trait SensorDriver {
    fn meta(&self) -> &'static SensorMeta;

    /// One sampling step: return the measurement to queue, if any. The
    /// scaffold stubs return `None` on device (no hardware yet); driver PRs
    /// fill the body in behind the same signature.
    fn poll(&mut self) -> Option<Measurement>;

    /// Applies a control command from core 0. Returns the ack to queue if
    /// the command concerns this driver; the default is "not mine" (the
    /// partition-level commands — park/resume/shutdown — are the system's).
    fn on_control(&mut self, cmd: Control) -> Option<Ack> {
        let _ = cmd;
        None
    }
}

/// The core-1 composition root: every driver compiled in via its flag,
/// named fields, no vtables — static dispatch keeps the real-time loop's
/// worst case inspectable (spec §Architecture). Assembled once at boot;
/// never rebuilt.
pub struct Core1System {
    /// Driver count for this compile — the build-time arm count of
    /// [`Self::build_registry`]. A mismatch between the two is a flag-
    /// wiring bug; `cargo build --all-features` compiles every arm
    /// together so the pair cannot drift silently.
    pub expected_driver_count: usize,
    #[cfg(feature = "sensor-decibel")]
    pub sensors_decibel: sensors::decibel::DecibelStub,
    #[cfg(feature = "sensor-pax")]
    pub sensors_pax: sensors::pax::PaxStub,
    #[cfg(feature = "sensor-bme280")]
    pub sensors_bme280: sensors::bme280::Bme280Stub,
    #[cfg(feature = "sensor-sht4x")]
    pub sensors_sht4x: sensors::sht4x::Sht4xStub,
    #[cfg(feature = "sensor-scd4x")]
    pub sensors_scd4x: sensors::scd4x::Scd4xStub,
    #[cfg(feature = "sensor-pms5003")]
    pub sensors_pms5003: sensors::pms5003::Pms5003Stub,
    #[cfg(feature = "sensor-bh1750")]
    pub sensors_bh1750: sensors::bh1750::Bh1750Stub,
    /// Deep-sleep coordination: sampling paused by core 0.
    parked: bool,
    /// Measurements dropped because the measurement queue was full —
    /// back-pressure is counted, never a blocking stall.
    pub samples_dropped: u32,
}

impl Default for Core1System {
    fn default() -> Self {
        Self::build_registry()
    }
}

impl Core1System {
    /// Builds the partition from the compiled-in drivers only. A disabled
    /// flag means the driver does not exist here — there is no "disabled"
    /// state at runtime (spec §Flags).
    pub fn build_registry() -> Self {
        Core1System {
            expected_driver_count: Self::expected_driver_count(),
            #[cfg(feature = "sensor-decibel")]
            sensors_decibel: sensors::decibel::DecibelStub::new(),
            #[cfg(feature = "sensor-pax")]
            sensors_pax: sensors::pax::PaxStub::new(),
            #[cfg(feature = "sensor-bme280")]
            sensors_bme280: sensors::bme280::Bme280Stub::new(),
            #[cfg(feature = "sensor-sht4x")]
            sensors_sht4x: sensors::sht4x::Sht4xStub::new(),
            #[cfg(feature = "sensor-scd4x")]
            sensors_scd4x: sensors::scd4x::Scd4xStub::new(),
            #[cfg(feature = "sensor-pms5003")]
            sensors_pms5003: sensors::pms5003::Pms5003Stub::new(),
            #[cfg(feature = "sensor-bh1750")]
            sensors_bh1750: sensors::bh1750::Bh1750Stub::new(),
            parked: false,
            samples_dropped: 0,
        }
    }

    /// Driver count for this compile — mirrors the cfg arms of
    /// [`Self::build_registry`].
    pub fn expected_driver_count() -> usize {
        [
            cfg!(feature = "sensor-decibel"),
            cfg!(feature = "sensor-pax"),
            cfg!(feature = "sensor-bme280"),
            cfg!(feature = "sensor-sht4x"),
            cfg!(feature = "sensor-scd4x"),
            cfg!(feature = "sensor-pms5003"),
            cfg!(feature = "sensor-bh1750"),
        ]
        .into_iter()
        .filter(|enabled| *enabled)
        .count()
    }

    /// Applies one control command from core 0. Partition-level commands
    /// (park/resume/shutdown) are the system's; calibration and thresholds
    /// forward to the owning driver. Returns the ack to queue.
    fn apply_control(&mut self, command: Control) -> Ack {
        match command {
            Control::ParkSampling => {
                self.parked = true;
                Ack::Parked
            }
            Control::ResumeSampling => {
                self.parked = false;
                Ack::Resumed
            }
            Control::Shutdown => Ack::ShutdownQueued,
            #[cfg(feature = "sensor-decibel")]
            command @ (Control::SetCalibrationDb(_)
            | Control::SetOverloadThresholdDb(_)) => self
                .sensors_decibel
                .on_control(command)
                .unwrap_or(Ack::NoHandler),
            #[cfg(not(feature = "sensor-decibel"))]
            Control::SetCalibrationDb(_) | Control::SetOverloadThresholdDb(_) => {
                Ack::NoHandler
            }
        }
    }
}

impl SamplingPartition for Core1System {
    fn tick(&mut self, link: &mut Core1Link) -> u32 {
        if let Some(command) = link.drain_control() {
            let ack = self.apply_control(command);
            // Ack queue full → the ack is dropped by design; the command
            // still applied.
            let _ = link.post_ack(ack);
        }

        if self.parked {
            return 0;
        }

        // With no sensor compiled, no poll! arm increments the counter —
        // the allow is honest about that combination.
        #[allow(unused_mut)]
        let mut queued = 0;
        // With no sensor compiled, every poll! arm is cfg'd out —
        // the allow is honest about that combination.
        #[allow(unused_macros)]
        macro_rules! poll {
            ($driver:expr) => {
                if let Some(measurement) = $driver.poll() {
                    if link.post_measurement(measurement) {
                        queued += 1;
                    } else {
                        self.samples_dropped += 1;
                    }
                }
            };
        }
        #[cfg(feature = "sensor-decibel")]
        poll!(self.sensors_decibel);
        #[cfg(feature = "sensor-pax")]
        poll!(self.sensors_pax);
        #[cfg(feature = "sensor-bme280")]
        poll!(self.sensors_bme280);
        #[cfg(feature = "sensor-sht4x")]
        poll!(self.sensors_sht4x);
        #[cfg(feature = "sensor-scd4x")]
        poll!(self.sensors_scd4x);
        #[cfg(feature = "sensor-pms5003")]
        poll!(self.sensors_pms5003);
        #[cfg(feature = "sensor-bh1750")]
        poll!(self.sensors_bh1750);
        queued
    }
}

#[cfg(test)]
mod system_tests {
    use super::*;
    use crate::boundary::leak_split;

    #[test]
    fn registry_builds_with_expected_driver_count() {
        let system = Core1System::build_registry();
        assert_eq!(
            system.expected_driver_count,
            Core1System::expected_driver_count()
        );
        assert!(system.expected_driver_count <= 7);
    }

    #[test]
    fn parked_partition_acks_and_queues_nothing() {
        let (mut link, mut remote) = leak_split();
        let mut system = Core1System::build_registry();

        assert!(remote.post_control(Control::ParkSampling));
        system.tick(&mut link);
        assert_eq!(remote.drain_measurements().count(), 0);
        assert_eq!(remote.drain_acks().next(), Some(Ack::Parked));

        assert!(remote.post_control(Control::ResumeSampling));
        system.tick(&mut link);
        assert_eq!(remote.drain_acks().next(), Some(Ack::Resumed));
    }

    #[test]
    fn calibration_routes_by_compiled_flags() {
        let (mut link, mut remote) = leak_split();
        let mut system = Core1System::build_registry();
        assert!(remote.post_control(Control::SetCalibrationDb(3.0)));
        system.tick(&mut link);

        #[cfg(feature = "sensor-decibel")]
        {
            assert_eq!(remote.drain_acks().next(), Some(Ack::CalibrationSet));
            assert_eq!(system.sensors_decibel.calibration_offset_db, 3.0);
        }
        #[cfg(not(feature = "sensor-decibel"))]
        {
            assert_eq!(remote.drain_acks().next(), Some(Ack::NoHandler));
        }
    }
}
