//! Core 0 partition — Wi-Fi/network stack, transports, ops
//! (latency-tolerant).
//!
//! Charter (spec §Architecture): this core may stall, retry, or back off
//! freely; nothing it does may ever block the sensing core. It drains the
//! measurement queue into the active transport, posts control commands
//! (calibration, park/resume for deep sleep) through the mailbox, and
//! services ops cadences. The composition root (`Core0System`) lands with
//! the cross-core primitive commit (crate::cores).

pub mod ops {
    //! Ops capability stubs — OTA, provisioning, sleep, logging, status.

    #[cfg(feature = "deep-sleep")]
    pub mod deep_sleep;
    #[cfg(feature = "logging-defmt")]
    pub mod logging;
    #[cfg(feature = "ota")]
    pub mod ota;
    #[cfg(feature = "provisioning")]
    pub mod provisioning;
    #[cfg(feature = "ops-status")]
    pub mod status;
}

pub mod transports {
    //! Ingest transport stubs — config selects the active one at runtime;
    //! these flags decide what is compiled in at all.

    #[cfg(feature = "transport-http")]
    pub mod http;
    #[cfg(feature = "transport-mqtt")]
    pub mod mqtt;
    #[cfg(feature = "transport-mtls")]
    pub mod mtls;
}

#[cfg(feature = "deep-sleep")]
use crate::boundary::Control;
use crate::boundary::{Core0Link, OpsPartition};
use crate::mailbox::ControlMailbox;

#[cfg(feature = "deep-sleep")]
use crate::core0::ops::deep_sleep::{Decision, SleepCoordinator};
#[cfg(feature = "logging-defmt")]
use crate::core0::ops::logging::LoggingStub;
#[cfg(feature = "ota")]
use crate::core0::ops::ota::OtaStub;
#[cfg(feature = "provisioning")]
use crate::core0::ops::provisioning::ProvisioningStub;
#[cfg(feature = "ops-status")]
use crate::core0::ops::status::StatusHeartbeat;
#[cfg(feature = "transport-http")]
use crate::core0::transports::http::HttpTransportStub;
#[cfg(feature = "transport-mqtt")]
use crate::core0::transports::mqtt::MqttTransportStub;

/// The core-0 composition root: the active transport and the ops cadences
/// compiled in via their flags. Owns the writing side of the control
/// mailbox — it is the only core that commands core 1 (spec §Architecture).
pub struct Core0System {
    /// Which compiled transport handles drained measurements. Only
    /// compiled-in transports are valid; a config value naming an absent
    /// transport drops measurements observably (counted, never a panic).
    pub active_transport: &'static str,
    #[cfg(feature = "ota")]
    pub ota: Option<OtaStub>,
    #[cfg(feature = "provisioning")]
    pub provisioning: ProvisioningStub,
    #[cfg(feature = "deep-sleep")]
    pub sleep_coordinator: SleepCoordinator,
    #[cfg(feature = "logging-defmt")]
    pub logging: LoggingStub,
    #[cfg(feature = "ops-status")]
    pub status: StatusHeartbeat,
    #[cfg(feature = "transport-http")]
    pub http: HttpTransportStub,
    #[cfg(feature = "transport-mqtt")]
    pub mqtt: MqttTransportStub,
    /// Measurements handed to the active transport since boot.
    pub measurements_published: u32,
    /// Measurements dropped for lack of a compiled active transport.
    pub measurements_dropped: u32,
    /// Control commands accepted by the channel to core 1 since boot.
    pub commands_posted: u32,
    /// Acks received from core 1 since boot (they release the mailbox).
    pub acks_received: u32,
    /// The mailbox discipline: at most one command outstanding; new
    /// intents coalesce at the producer until core 1 acks.
    pub mailbox: ControlMailbox,
}

impl Default for Core0System {
    fn default() -> Self {
        Self::new()
    }
}

impl Core0System {
    /// Builds the partition with the first compiled-in transport as the
    /// active one — http preferred (it is the working server path).
    pub fn new() -> Self {
        let active_transport: &'static str = if cfg!(feature = "transport-http") {
            "http"
        } else if cfg!(feature = "transport-mqtt") {
            "mqtt"
        } else {
            "none"
        };
        Core0System {
            active_transport,
            #[cfg(feature = "ota")]
            ota: None,
            #[cfg(feature = "provisioning")]
            provisioning: ProvisioningStub::new(),
            #[cfg(feature = "deep-sleep")]
            sleep_coordinator: SleepCoordinator::new(4, 2),
            #[cfg(feature = "logging-defmt")]
            logging: LoggingStub::new(),
            #[cfg(feature = "ops-status")]
            status: StatusHeartbeat::new(),
            #[cfg(feature = "transport-http")]
            http: HttpTransportStub::new(),
            #[cfg(feature = "transport-mqtt")]
            mqtt: MqttTransportStub::new(),
            measurements_published: 0,
            measurements_dropped: 0,
            commands_posted: 0,
            acks_received: 0,
            mailbox: ControlMailbox::new(),
        }
    }

    /// Builder-style OTA attachment (kept out of `new` so the no-ota
    /// compile has no dead state).
    #[cfg(feature = "ota")]
    pub fn with_ota(mut self, ota: OtaStub) -> Self {
        self.ota = Some(ota);
        self
    }

    /// Overrides the active transport (the runtime-config half of the
    /// flag system). Values naming a transport that is not compiled in
    /// are accepted but drop measurements — a config error surfaces as
    /// an observable, not a panic.
    pub fn set_active_transport(&mut self, transport: &'static str) {
        self.active_transport = transport;
    }

    /// Runs the ops cadences that concern core 1 through the control
    /// mailbox. Deep sleep is the first user: the coordinator on THIS
    /// core decides, core 1 is told — never the reverse (charter: core 1
    /// has no say in power; it is only parked). OTA and status beats run
    /// regardless of deep-sleep — status is on by default.
    fn tick_ops(&mut self, link: &mut Core0Link) {
        #[cfg(feature = "deep-sleep")]
        match self.sleep_coordinator.advance() {
            Decision::Park => {
                self.post_control_coalesced(link, Control::ParkSampling);
            }
            Decision::Resume => {
                self.post_control_coalesced(link, Control::ResumeSampling);
            }
            Decision::None => {}
        }

        // Without deep-sleep there is no control producer on this core —
        // acknowledge the link explicitly rather than renaming the field.
        #[cfg(not(feature = "deep-sleep"))]
        let _ = link;

        #[cfg(feature = "ota")]
        if let Some(ota) = &mut self.ota {
            ota.tick();
        }

        #[cfg(feature = "ops-status")]
        {
            self.status.beat();
        }
    }

    /// Posts through the mailbox discipline: coalesce when a previous
    /// command is unacked (latest-wins at the producer — see
    /// crate::mailbox), count only what the channel accepted.
    #[cfg(feature = "deep-sleep")]
    fn post_control_coalesced(&mut self, link: &mut Core0Link, command: Control) {
        if self.mailbox.can_post() && link.post_control(command) {
            self.mailbox.mark_posted();
            self.commands_posted += 1;
        }
    }
}

impl OpsPartition for Core0System {
    fn tick(&mut self, link: &mut Core0Link) -> u32 {
        // Acks first: core 1 applied a command — release the mailbox
        // before this tick's cadence decides anything new.
        for _ack in link.drain_acks() {
            self.acks_received += 1;
            self.mailbox.on_ack();
        }

        self.tick_ops(link);

        // With no transport compiled, no arm increments the counter —
        // the allow is honest about that combination.
        #[allow(unused_mut)]
        let mut drained = 0;
        for measurement in link.drain_measurements() {
            match self.active_transport {
                #[cfg(feature = "transport-http")]
                "http" => {
                    self.http.publish(&measurement);
                    self.measurements_published += 1;
                    drained += 1;
                }
                #[cfg(feature = "transport-mqtt")]
                "mqtt" => {
                    self.mqtt.publish(&measurement);
                    self.measurements_published += 1;
                    drained += 1;
                }
                _other => {
                    let _ = measurement;
                    self.measurements_dropped += 1;
                }
            }
        }
        drained
    }
}

#[cfg(test)]
mod system_tests {
    use super::*;
    use crate::boundary::{Measurement, leak_split};

    #[test]
    fn default_transport_is_a_compiled_in_one() {
        let system = Core0System::new();
        let valid = [
            "none",
            #[cfg(feature = "transport-http")]
            "http",
            #[cfg(feature = "transport-mqtt")]
            "mqtt",
        ];
        assert!(valid.contains(&system.active_transport));
    }

    #[test]
    fn drains_measurements_into_the_active_transport() {
        let (mut core1_link, mut core0_link) = leak_split();
        core1_link.post_measurement(Measurement::Pax { occupancy_count: 7 });
        core1_link.post_measurement(Measurement::Co2 { ppm: 650 });

        let mut system = Core0System::new();
        let has_any_transport =
            cfg!(feature = "transport-http") || cfg!(feature = "transport-mqtt");
        let drained = system.tick(&mut core0_link);

        if has_any_transport {
            assert_eq!(drained, 2);
            assert_eq!(system.measurements_published, 2);
            assert_eq!(system.measurements_dropped, 0);
        } else {
            assert_eq!(drained, 0);
            assert_eq!(system.measurements_dropped, 2);
        }
    }

    #[test]
    fn unknown_transport_config_drops_observably() {
        let (mut core1_link, mut core0_link) = leak_split();
        core1_link.post_measurement(Measurement::Light { lux: 1.0 });

        let mut system = Core0System::new();
        system.set_active_transport("carrier-pigeon");
        assert_eq!(system.tick(&mut core0_link), 0);
        assert_eq!(system.measurements_dropped, 1);
    }

    #[cfg(feature = "deep-sleep")]
    #[test]
    fn sleep_cadence_posts_control_commands_to_core1() {
        use crate::boundary::Ack;

        let (mut core1_link, mut core0_link) = leak_split();
        let mut system = Core0System::new(); // parks after 4 awake ticks

        for _ in 0..4 {
            system.tick(&mut core0_link);
        }
        assert_eq!(core1_link.drain_control(), Some(Control::ParkSampling));
        assert_eq!(system.commands_posted, 1);

        // Core 1 applies Park and acks; core 0 drains the ack, releasing
        // the mailbox for the next command.
        assert!(core1_link.post_ack(Ack::Parked));

        // The sleep window runs 2 ticks, then the coordinator resumes.
        for _ in 0..2 {
            system.tick(&mut core0_link);
        }
        assert_eq!(core1_link.drain_control(), Some(Control::ResumeSampling));
        assert_eq!(system.commands_posted, 2);
    }

    #[cfg(feature = "deep-sleep")]
    #[test]
    fn an_unacked_command_coalesces_instead_of_queueing() {
        let (mut _core1_link, mut core0_link) = leak_split();
        let mut system = Core0System::new();

        for _ in 0..4 {
            system.tick(&mut core0_link);
        }
        assert_eq!(system.commands_posted, 1);
        assert!(system.mailbox.is_outstanding());

        // More ticks: the coordinator decides again, but the mailbox is
        // full — no second command enters the channel until core 1 acks
        // (latest-wins at the producer; the channel never backlogs).
        for _ in 0..4 {
            system.tick(&mut core0_link);
        }
        assert_eq!(system.commands_posted, 1);
        assert_eq!(system.acks_received, 0);
    }
}
