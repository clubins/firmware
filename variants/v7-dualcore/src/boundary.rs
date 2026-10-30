//! The cross-core boundary — V7's architectural primitive.
//!
//! Strict partitioning (spec §Architecture): core 1 owns sensing + DSP,
//! core 0 owns Wi-Fi/transports/ops. The ONLY channel between them lives in
//! this module: a bounded SPSC queue of [`Measurement`]s (core 1 → core 0),
//! a tiny SPSC queue of [`Ack`]s (core 1 → core 0), and the control queue
//! (core 0 → core 1) whose [`crate::mailbox`] discipline turns it into a
//! latest-wins mailbox. No shared mutable state beyond the queues, no
//! locks — a network-side fault can stall publishing but never sampling:
//! at worst the measurement queue fills and core 1 counts drops.
//!
//! Synchronization: the queues are `heapless::spsc` (atomic cursors; the
//! crate's internal unsafety is upstream's, audited, not ours). Each queue
//! is split exactly once at boot — the producer half lives on one core,
//! the consumer half on the other, and the halves never migrate. That
//! split-once discipline is what makes a lock-free SPSC sound cross-core;
//! the typed [`Core1Link`]/[`Core0Link`] enforce it by construction: there
//! is no way to obtain a second handle to either half.

use alloc::boxed::Box;

use heapless::spsc::{Consumer, Producer, Queue};

// heapless's SPSC queue reserves one slot to distinguish full from empty —
// a `Queue<T, N>` holds N−1 elements (heapless 0.8.0, src/spsc.rs:106). The
// *_Q_CAP constants below are *usable* capacities, so every queue is
// declared one slot larger than its constant. Keep the +1 in every type
// that repeats a capacity — the constants alone would under-allocate.

/// Capacity of the measurement queue, in slots. At the 1 Hz report cadence
/// this absorbs ~64 s of a wedged network core before core 1 starts
/// counting drops.
pub const MEASUREMENT_Q_CAP: usize = 64;

/// Capacity of the ack queue, in slots (acks are 1:1 with commands, so a
/// few slots absorb a burst of commands applied in one window).
pub const ACK_Q_CAP: usize = 4;

/// Capacity of the control queue — the "small control mailbox". The
/// producer-side discipline (crate::mailbox) keeps at most one command
/// outstanding, so the second slot only exists so the producer's post and
/// the consumer's take never need to synchronize on the same slot.
pub const CONTROL_Q_CAP: usize = 2;

/// Sensor identity placeholder — the real UUID lives in NVS at runtime; the
/// committed defaults never carry secrets or device identity.
pub const PLACEHOLDER_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// api/mqtt/topics.md: data messages go out at QoS 1 (the server dedups by
/// `boot_id` + per-topic `seq`).
pub const MQTT_DATA_QOS: u8 = 1;

/// api/mqtt/topics.md: the Status message published on connect is retained.
pub const MQTT_STATUS_RETAINED: bool = true;

/// One measurement crossing from the sensing core to the network core.
/// Payload fields mirror the ingest contracts (server-infra/ingest/service/
/// *.go) at the model level; the wire envelope (`sensor_id`, `apikey`,
/// `time`, `measurement_id`) is transport-level and arrives with the
/// transport PRs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Measurement {
    /// A-weighted and C-weighted levels for one window (decibel.md: LAeq,
    /// LCeq; `overload` flags a clipped window).
    Decibel {
        dba: f32,
        dbc: f32,
        overload: bool,
        window_ms: u16,
    },
    /// Occupancy detection count (PIR front-end; motion detections, not a
    /// head count — spec §Sensor Expansion).
    Pax { occupancy_count: u32 },
    /// Temperature (°C), humidity (%), pressure (hPa) — BME280/SHT4x.
    /// The server's precipitation/rain/wind fields are virtual Open-Meteo
    /// values and are never emitted from hardware.
    Weather {
        temperature_c: f32,
        humidity_pct: f32,
        pressure_hpa: f32,
    },
    /// CO₂ (ppm) — SCD4x. New ingest type: emitted per the
    /// one-topic-per-type rule; the server receiver is a backend follow-up.
    Co2 { ppm: u16 },
    /// PM2.5 (µg/m³) — PMS5003. New ingest type (same rule).
    Pm25 { ug_m3: u16 },
    /// Ambient light (lx) — BH1750. New ingest type (same rule).
    Light { lux: f32 },
}

impl Measurement {
    /// The ingest type this measurement publishes under — one endpoint and
    /// one topic per type (HTTP ingest + api/mqtt/topics.md).
    pub const fn ingest_type(&self) -> &'static str {
        match self {
            Measurement::Decibel { .. } => "decibel",
            Measurement::Pax { .. } => "pax",
            Measurement::Weather { .. } => "weather",
            Measurement::Co2 { .. } => "co2",
            Measurement::Pm25 { .. } => "pm25",
            Measurement::Light { .. } => "light",
        }
    }
}

/// Control commands, core 0 → core 1 (the mailbox channel; latest wins at
/// the producer — see `crate::mailbox`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    /// Per-unit decibel calibration offset (dB) — loaded from NVS at
    /// runtime per decibel.md; core 0 forwards it.
    SetCalibrationDb(f32),
    /// Overload detection threshold (dB) — decibel.md.
    SetOverloadThresholdDb(f32),
    /// Deep-sleep coordination: stop polling drivers (core 1 idles).
    ParkSampling,
    /// Resume the sampling loop.
    ResumeSampling,
    /// Orderly shutdown of the sensing loop (driver PRs: flush first).
    Shutdown,
}

/// Acknowledgements, core 1 → core 0 (tiny SPSC queue).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ack {
    Parked,
    Resumed,
    CalibrationSet,
    OverloadThresholdSet,
    ShutdownQueued,
    /// No compiled driver claimed the command (e.g. decibel disabled).
    NoHandler,
}

/// A fixed-capacity route/topic string. Cross-core and transport code never
/// allocates; 96 bytes covers the longest MQTT topic with a full UUID.
#[derive(Debug, Clone)]
pub struct RouteBuf {
    len: usize,
    bytes: [u8; ROUTE_CAP],
}

pub const ROUTE_CAP: usize = 96;

impl RouteBuf {
    pub const fn new() -> Self {
        RouteBuf {
            len: 0,
            bytes: [0; ROUTE_CAP],
        }
    }

    /// Appends a fragment. Route parts are contract constants well under the
    /// capacity; debug builds panic on overflow instead of truncating.
    pub fn push_str(&mut self, s: &str) {
        debug_assert!(s.len() <= ROUTE_CAP - self.len, "route buffer overflow");
        let n = s.len().min(ROUTE_CAP - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(s.as_bytes());
        self.len += n;
    }

    pub fn as_str(&self) -> &str {
        // Every byte was copied from a &str, so the slice is valid UTF-8 by
        // construction; the fallback is unreachable but keeps this total.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }
}

impl Default for RouteBuf {
    fn default() -> Self {
        Self::new()
    }
}

/// The traits each partition implements — the launch path and runtimes
/// program against these (docs/plan/build-dag.md).
///
/// The sensing + DSP partition (core 1) steps its drivers in a fixed
/// order; everything it offers the outside world is [`Control`] in and
/// [`Measurement`]/[`Ack`] out.
pub trait SamplingPartition {
    /// One sampling step: drains control commands, polls drivers, enqueues
    /// measurements. Returns how many measurements were queued this step.
    fn tick(&mut self, link: &mut Core1Link) -> u32;
}

/// The network + ops partition (core 0). Steps transports and ops
/// cadences; a fault here (Wi-Fi down, broker unreachable) may lose
/// measurements but can never block the sensing partition.
pub trait OpsPartition {
    /// One ops step: posts control, drains the measurement queue into the
    /// active transport, runs ops cadences. Returns how many measurements
    /// were drained this step.
    fn tick(&mut self, link: &mut Core0Link) -> u32;
}

/// The sensing core's half of the boundary: enqueue measurements, enqueue
/// acks, take control. Owns the producer halves of the two core 1 → core 0
/// queues and the consumer half of the control queue; there is deliberately
/// no way to reach core-0 state from here — the partition charter is
/// enforced by the type.
pub struct Core1Link {
    measurements_in: Producer<'static, Measurement, { MEASUREMENT_Q_CAP + 1 }>,
    acks_in: Producer<'static, Ack, { ACK_Q_CAP + 1 }>,
    control_out: Consumer<'static, Control, { CONTROL_Q_CAP + 1 }>,
}

impl Core1Link {
    /// Queues a measurement for core 0; `false` means the queue is full and
    /// the measurement is dropped (back-pressure is counted, never a
    /// blocking stall).
    pub fn post_measurement(&mut self, measurement: Measurement) -> bool {
        self.measurements_in.enqueue(measurement).is_ok()
    }

    /// Queues an ack for core 0; `false` means the ack queue is full (the
    /// command still applied — acks are best-effort telemetry).
    pub fn post_ack(&mut self, ack: Ack) -> bool {
        self.acks_in.enqueue(ack).is_ok()
    }

    /// Takes the pending control command, if core 0 posted one.
    pub fn drain_control(&mut self) -> Option<Control> {
        self.control_out.dequeue()
    }
}

/// The network/ops core's half of the boundary: post control, drain
/// measurements and acks. Nothing here can touch core-1 state.
pub struct Core0Link {
    measurements_out: Consumer<'static, Measurement, { MEASUREMENT_Q_CAP + 1 }>,
    acks_out: Consumer<'static, Ack, { ACK_Q_CAP + 1 }>,
    control_in: Producer<'static, Control, { CONTROL_Q_CAP + 1 }>,
}

impl Core0Link {
    /// Posts a control command for core 1; `false` means the mailbox slot
    /// is busy (the producer-side discipline in `crate::mailbox` coalesces
    /// instead of queueing).
    pub fn post_control(&mut self, command: Control) -> bool {
        self.control_in.enqueue(command).is_ok()
    }

    /// Drains every queued measurement, oldest first.
    pub fn drain_measurements(
        &mut self,
    ) -> Drain<'_, Measurement, { MEASUREMENT_Q_CAP + 1 }> {
        Drain {
            consumer: &mut self.measurements_out,
        }
    }

    /// Drains every queued ack.
    pub fn drain_acks(&mut self) -> Drain<'_, Ack, { ACK_Q_CAP + 1 }> {
        Drain {
            consumer: &mut self.acks_out,
        }
    }
}

/// Draining iterator over one consumer half of a queue.
pub struct Drain<'a, T: 'static, const N: usize> {
    consumer: &'a mut Consumer<'static, T, N>,
}

impl<T: 'static, const N: usize> Iterator for Drain<'_, T, N> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.consumer.dequeue()
    }
}

/// Statically allocates the boundary and splits it into the two per-core
/// links — the single choke point where the cores meet. Call exactly once
/// at boot; on device this runs before the core-1 stack is started. Each
/// queue is split once, forever: the producer halves go to their owning
/// core, the consumer halves to theirs.
pub fn leak_split() -> (Core1Link, Core0Link) {
    let measurements: &'static mut Queue<Measurement, { MEASUREMENT_Q_CAP + 1 }> =
        Box::leak(Box::new(Queue::new()));
    let acks: &'static mut Queue<Ack, { ACK_Q_CAP + 1 }> =
        Box::leak(Box::new(Queue::new()));
    let control: &'static mut Queue<Control, { CONTROL_Q_CAP + 1 }> =
        Box::leak(Box::new(Queue::new()));

    let (measurements_in, measurements_out) = measurements.split();
    let (acks_in, acks_out) = acks.split();
    let (control_in, control_out) = control.split();

    (
        Core1Link {
            measurements_in,
            acks_in,
            control_out,
        },
        Core0Link {
            measurements_out,
            acks_out,
            control_in,
        },
    )
}

/// HTTP ingest route for one measurement type:
/// `<endpoint>/v1/sensors/<type>` (server-infra/ingest/service/). The
/// endpoint base is runtime config; this is the path part.
pub fn http_route(ingest_type: &str) -> RouteBuf {
    let mut route = RouteBuf::new();
    route.push_str("/v1/sensors/");
    route.push_str(ingest_type);
    route
}

/// MQTT v2 data topic for one measurement type:
/// `v2/sensors/<uuid>/data/<type>` (api/mqtt/topics.md — one topic per
/// type, QoS 1).
pub fn mqtt_topic(sensor_uuid: &str, ingest_type: &str) -> RouteBuf {
    let mut topic = RouteBuf::new();
    topic.push_str("v2/sensors/");
    topic.push_str(sensor_uuid);
    topic.push_str("/data/");
    topic.push_str(ingest_type);
    topic
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_measurement_maps_to_its_ingest_type() {
        let cases = [
            (
                Measurement::Decibel {
                    dba: 41.5,
                    dbc: 43.0,
                    overload: false,
                    window_ms: 125,
                },
                "decibel",
            ),
            (Measurement::Pax { occupancy_count: 2 }, "pax"),
            (
                Measurement::Weather {
                    temperature_c: 21.0,
                    humidity_pct: 55.0,
                    pressure_hpa: 1013.0,
                },
                "weather",
            ),
            (Measurement::Co2 { ppm: 812 }, "co2"),
            (Measurement::Pm25 { ug_m3: 14 }, "pm25"),
            (Measurement::Light { lux: 310.5 }, "light"),
        ];
        for (m, want) in cases {
            assert_eq!(m.ingest_type(), want);
        }
    }

    #[test]
    fn http_routes_are_path_exact_per_type() {
        for t in [
            "decibel", "pax", "weather", "co2", "pm25", "light", "status",
        ] {
            assert_eq!(http_route(t).as_str(), format!("/v1/sensors/{t}"));
        }
    }

    #[test]
    fn mqtt_topics_are_topic_exact_per_type() {
        for t in [
            "decibel", "pax", "weather", "co2", "pm25", "light", "status",
        ] {
            assert_eq!(
                mqtt_topic(PLACEHOLDER_UUID, t).as_str(),
                format!("v2/sensors/{PLACEHOLDER_UUID}/data/{t}")
            );
        }
    }

    #[test]
    fn mqtt_contract_constants_match_topics_md() {
        // Compile-time contract pin (api/mqtt/topics.md): const asserts —
        // a runtime `assert!(CONST)` would be both a lint failure and
        // tautological; this fails at build time if the constant drifts.
        const {
            assert!(MQTT_DATA_QOS == 1, "QoS 1 per api/mqtt/topics.md");
            assert!(MQTT_STATUS_RETAINED, "connect status is retained");
        }
    }

    #[test]
    fn route_buf_appends_fragments() {
        let mut r = RouteBuf::new();
        r.push_str("v2/sensors/");
        r.push_str(PLACEHOLDER_UUID);
        r.push_str("/data/");
        r.push_str("decibel");
        assert_eq!(
            r.as_str(),
            "v2/sensors/00000000-0000-0000-0000-000000000000/data/decibel"
        );
    }

    #[test]
    fn leak_split_gives_core1_a_producer_and_core0_a_consumer() {
        let (mut core1, mut core0) = leak_split();
        assert!(core0.drain_measurements().next().is_none());

        assert!(core1.post_measurement(Measurement::Pax { occupancy_count: 1 }));
        assert!(core1.post_measurement(Measurement::Light { lux: 5.0 }));
        assert!(core1.post_ack(Ack::Resumed));

        let got: std::vec::Vec<Measurement> = core0.drain_measurements().collect();
        assert_eq!(
            got.as_slice(),
            &[
                Measurement::Pax { occupancy_count: 1 },
                Measurement::Light { lux: 5.0 },
            ]
        );
        let acks: std::vec::Vec<Ack> = core0.drain_acks().collect();
        assert_eq!(acks.as_slice(), &[Ack::Resumed]);
    }

    #[test]
    fn measurement_queue_back_pressures_by_counting_drops() {
        let (mut core1, mut core0) = leak_split();
        // Fill the measurement queue to capacity; the queue holds all of
        // them, the next post fails (the caller counts the drop), and the
        // drain returns every value in order — no loss inside capacity.
        for i in 0..MEASUREMENT_Q_CAP {
            assert!(
                core1.post_measurement(Measurement::Co2 { ppm: i as u16 }),
                "slot {i} of {MEASUREMENT_Q_CAP} should be free"
            );
        }
        assert!(
            !core1.post_measurement(Measurement::Co2 { ppm: 9999 }),
            "full queue must reject, not block or wrap"
        );
        let got: std::vec::Vec<Measurement> = core0.drain_measurements().collect();
        assert_eq!(got.len(), MEASUREMENT_Q_CAP);
        assert_eq!(got[0], Measurement::Co2 { ppm: 0 });
        assert_eq!(
            got[MEASUREMENT_Q_CAP - 1],
            Measurement::Co2 {
                ppm: (MEASUREMENT_Q_CAP - 1) as u16
            }
        );
    }

    #[test]
    fn control_flows_from_core0_to_core1_through_the_split() {
        let (mut core1, mut core0) = leak_split();
        assert_eq!(core1.drain_control(), None);
        assert!(core0.post_control(Control::SetOverloadThresholdDb(115.0)));
        assert_eq!(
            core1.drain_control(),
            Some(Control::SetOverloadThresholdDb(115.0))
        );
        assert_eq!(core1.drain_control(), None);
    }
}
