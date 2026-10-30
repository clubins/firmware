//! The statically-allocated dataflow graph and its pump.
//!
//! [`Graph`] is the whole V6 system for the compiled feature set: sensor
//! nodes and the status heartbeat produce [`Measurement`]s into rings, the
//! router fans every emission out to every compiled transport drain, and
//! the pump ([`Graph::step`]) advances the DAG in a fixed order —
//! producers → stages → routing → drains → ops ticks. Nothing allocates,
//! nothing locks, nothing blocks: DMA engines and the ISR push into rings
//! non-blockingly (losses counted), the pump drains rings to completion.
//!
//! The graph is a `const fn` composition of feature-gated nodes, so an
//! all-off build has essentially no fields, and each flag adds exactly its
//! node (plus the engine that feeds it) to the struct — the compile-time
//! footprint story checked by `budget.rs`.

#[cfg(any(feature = "sensor-decibel", feature = "sensor-pms5003"))]
use crate::dma::SimulatedDma;
#[cfg(feature = "sensor-pax")]
use crate::dma::SimulatedIsr;
use crate::measurement::Measurement;
#[cfg(any(
    feature = "ota",
    feature = "provisioning",
    feature = "deep-sleep",
    feature = "logging-defmt",
    feature = "ops-status"
))]
use crate::ops;
#[cfg(any(
    feature = "sensor-decibel",
    feature = "sensor-pax",
    feature = "sensor-bme280",
    feature = "sensor-sht4x",
    feature = "sensor-scd4x",
    feature = "sensor-pms5003",
    feature = "sensor-bh1750"
))]
use crate::sensors;
#[cfg(any(
    feature = "transport-http",
    feature = "transport-mqtt",
    feature = "transport-mtls"
))]
use crate::transports;
/// Cumulative system totals, snapshotted after each pump step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tick {
    /// DMA frames accepted into byte rings, all engines.
    pub frames_handed: u64,
    /// DMA frames lost to full rings, all engines (never entered).
    pub overruns: u64,
    /// Measurements emitted by sensor stages + the status heartbeat.
    pub measurements: u64,
    /// Successful routing pushes into transport input rings (counts once
    /// per transport — broadcast fans out).
    pub routed: u64,
    /// Measurements drained by transports since boot.
    pub drained: u64,
}

/// The statically-allocated dataflow graph for the compiled feature set.
pub struct Graph {
    /// Broadcast fan-out counter (how many routing pushes landed).
    routed_total: u64,

    // -- Sources: DMA-fed byte/segment rings + their engines -------------
    #[cfg(feature = "sensor-decibel")]
    pub decibel: sensors::decibel::Node,
    #[cfg(feature = "sensor-decibel")]
    pub decibel_dma: SimulatedDma,

    #[cfg(feature = "sensor-pms5003")]
    pub pms5003: sensors::pms5003::Node,
    #[cfg(feature = "sensor-pms5003")]
    pub pms5003_dma: SimulatedDma,

    #[cfg(feature = "sensor-pax")]
    pub pax: sensors::pax::Node,
    #[cfg(feature = "sensor-pax")]
    pub pax_isr: SimulatedIsr,

    // -- Sources: polled (I2C) -------------------------------------------
    #[cfg(feature = "sensor-bme280")]
    pub bme280: sensors::bme280::Node,
    #[cfg(feature = "sensor-sht4x")]
    pub sht4x: sensors::sht4x::Node,
    #[cfg(feature = "sensor-scd4x")]
    pub scd4x: sensors::scd4x::Node,
    #[cfg(feature = "sensor-bh1750")]
    pub bh1750: sensors::bh1750::Node,

    // -- Drains: transports ----------------------------------------------
    #[cfg(feature = "transport-http")]
    pub http: transports::http::Node,
    #[cfg(feature = "transport-mqtt")]
    pub mqtt: transports::mqtt::Node,
    #[cfg(feature = "transport-mtls")]
    pub mtls: transports::mtls::Node,

    // -- Ops --------------------------------------------------------------
    #[cfg(feature = "ops-status")]
    pub status: ops::status::Node,
    #[cfg(feature = "ota")]
    pub ota: ops::ota::Node,
    #[cfg(feature = "provisioning")]
    pub provisioning: ops::provisioning::Node,
    #[cfg(feature = "deep-sleep")]
    pub deep_sleep: ops::deep_sleep::Node,
    #[cfg(feature = "logging-defmt")]
    pub logging: ops::logging::Node,
}

impl Graph {
    /// The graph's total static footprint (rings + state) in bytes —
    /// `size_of::<Self>()` for the compiled feature set. This is the
    /// link-time memory budget claim, asserted in `budget.rs`.
    pub const BYTES: usize = core::mem::size_of::<Graph>();

    /// Build the graph for the compiled feature set (const: storable in a
    /// `static` via `cell::StaticCell`).
    pub const fn build() -> Self {
        Self {
            routed_total: 0,
            #[cfg(feature = "sensor-decibel")]
            decibel: sensors::decibel::Node::new(),
            #[cfg(feature = "sensor-decibel")]
            decibel_dma: SimulatedDma::new(crate::sensors::decibel::WINDOW_BYTES),
            #[cfg(feature = "sensor-pms5003")]
            pms5003: sensors::pms5003::Node::new(),
            #[cfg(feature = "sensor-pms5003")]
            pms5003_dma: SimulatedDma::new(crate::sensors::pms5003::FRAME_BYTES),
            #[cfg(feature = "sensor-pax")]
            pax: sensors::pax::Node::new(),
            #[cfg(feature = "sensor-pax")]
            pax_isr: SimulatedIsr::new(),
            #[cfg(feature = "sensor-bme280")]
            bme280: sensors::bme280::Node::new(),
            #[cfg(feature = "sensor-sht4x")]
            sht4x: sensors::sht4x::Node::new(),
            #[cfg(feature = "sensor-scd4x")]
            scd4x: sensors::scd4x::Node::new(),
            #[cfg(feature = "sensor-bh1750")]
            bh1750: sensors::bh1750::Node::new(),
            #[cfg(feature = "transport-http")]
            http: transports::http::Node::new(),
            #[cfg(feature = "transport-mqtt")]
            mqtt: transports::mqtt::Node::new(),
            #[cfg(feature = "transport-mtls")]
            mtls: transports::mtls::Node::new(),
            #[cfg(feature = "ops-status")]
            status: ops::status::Node::new(),
            #[cfg(feature = "ota")]
            ota: ops::ota::Node::new(),
            #[cfg(feature = "provisioning")]
            provisioning: ops::provisioning::Node::new(),
            #[cfg(feature = "deep-sleep")]
            deep_sleep: ops::deep_sleep::Node::new(),
            #[cfg(feature = "logging-defmt")]
            logging: ops::logging::Node::new(),
        }
    }

    /// Advance the DAG one step in dependency order:
    /// producers → stages → routing → drains → ops ticks.
    /// Returns the cumulative tick totals after the step.
    pub fn step(&mut self) -> Tick {
        // 1. Producers: DMA engines / ISR push into the input rings.
        #[cfg(feature = "sensor-decibel")]
        {
            // One 125 ms Fast window per step ≈ 8 windows/s of wall time.
            self.decibel_dma.transfer(&self.decibel.pcm, 1);
        }
        #[cfg(feature = "sensor-pms5003")]
        {
            // One UART frame per step (the sensor reports ~1/s).
            self.pms5003_dma.transfer(&self.pms5003.bytes, 1);
        }
        #[cfg(feature = "sensor-pax")]
        {
            // Two detections per step (motion bursts, in simulation).
            self.pax_isr.fire(&self.pax.events, 2);
        }

        // 2. Stages: fold inputs into measurements.
        #[cfg(feature = "sensor-decibel")]
        {
            self.decibel.poll();
        }
        #[cfg(feature = "sensor-pms5003")]
        {
            self.pms5003.poll();
        }
        #[cfg(feature = "sensor-pax")]
        {
            self.pax.poll();
        }
        #[cfg(feature = "sensor-bme280")]
        {
            self.bme280.poll();
        }
        #[cfg(feature = "sensor-sht4x")]
        {
            self.sht4x.poll();
        }
        #[cfg(feature = "sensor-scd4x")]
        {
            self.scd4x.poll();
        }
        #[cfg(feature = "sensor-bh1750")]
        {
            self.bh1750.poll();
        }
        #[cfg(feature = "ops-status")]
        {
            self.status.poll();
        }

        // 3. Routing: fan every emitted measurement out to every compiled
        // transport drain (broadcast; a full transport ring drops that
        // copy, counted by its ring).
        #[cfg(feature = "sensor-decibel")]
        {
            while let Some(m) = self.decibel.out.pop() {
                self.route(m);
            }
        }
        #[cfg(feature = "sensor-pms5003")]
        {
            while let Some(m) = self.pms5003.out.pop() {
                self.route(m);
            }
        }
        #[cfg(feature = "sensor-pax")]
        {
            while let Some(m) = self.pax.out.pop() {
                self.route(m);
            }
        }
        #[cfg(feature = "sensor-bme280")]
        {
            while let Some(m) = self.bme280.out.pop() {
                self.route(m);
            }
        }
        #[cfg(feature = "sensor-sht4x")]
        {
            while let Some(m) = self.sht4x.out.pop() {
                self.route(m);
            }
        }
        #[cfg(feature = "sensor-scd4x")]
        {
            while let Some(m) = self.scd4x.out.pop() {
                self.route(m);
            }
        }
        #[cfg(feature = "sensor-bh1750")]
        {
            while let Some(m) = self.bh1750.out.pop() {
                self.route(m);
            }
        }
        #[cfg(feature = "ops-status")]
        {
            while let Some(m) = self.status.out.pop() {
                self.route(m);
            }
        }

        // 4. Drains: transports consume their input rings (wire writes
        // land with the network PRs; here they count).
        #[cfg(feature = "transport-http")]
        {
            self.http.poll();
        }
        #[cfg(feature = "transport-mqtt")]
        {
            self.mqtt.poll();
        }
        #[cfg(feature = "transport-mtls")]
        {
            self.mtls.poll();
        }

        // 5. Ops ticks: nodes without a measurement path still advance.
        #[cfg(feature = "ota")]
        {
            self.ota.poll();
        }
        #[cfg(feature = "provisioning")]
        {
            self.provisioning.poll();
        }
        #[cfg(feature = "deep-sleep")]
        {
            self.deep_sleep.poll();
        }
        #[cfg(feature = "logging-defmt")]
        {
            self.logging.poll();
        }

        self.snapshot()
    }

    /// Fan one measurement out to every compiled transport.
    #[cfg_attr(
        any(
            not(any(
                feature = "sensor-decibel",
                feature = "sensor-pax",
                feature = "sensor-bme280",
                feature = "sensor-sht4x",
                feature = "sensor-scd4x",
                feature = "sensor-pms5003",
                feature = "sensor-bh1750",
                feature = "ops-status"
            )),
            not(any(
                feature = "transport-http",
                feature = "transport-mqtt",
                feature = "transport-mtls"
            ))
        ),
        allow(dead_code)
    )] // live only when a source and a transport are both compiled
    fn route(&mut self, m: Measurement) {
        #[cfg(feature = "transport-http")]
        if self.http.in_ring.push(m).is_ok() {
            self.routed_total += 1;
        }
        #[cfg(feature = "transport-mqtt")]
        if self.mqtt.in_ring.push(m).is_ok() {
            self.routed_total += 1;
        }
        #[cfg(feature = "transport-mtls")]
        if self.mtls.in_ring.push(m).is_ok() {
            self.routed_total += 1;
        }
        // Without any transport compiled there is nowhere to route; the
        // measurement is dropped here (the transports' absence is the
        // config choice, and the drop is visible in `routed`).
        #[cfg(not(any(feature = "transport-http", feature = "transport-mqtt")))]
        {
            let _ = m;
        }
    }

    /// Cumulative totals after the last step.
    pub fn tick(&self) -> Tick {
        self.snapshot()
    }

    fn snapshot(&self) -> Tick {
        Tick {
            frames_handed: self.frames_handed(),
            overruns: self.overruns(),
            measurements: self.measurements(),
            routed: self.routed_total,
            drained: self.drained(),
        }
    }

    /// Total DMA frames accepted, all engines.
    #[allow(unused_mut)] // a zero-feature build sums nothing
    pub fn frames_handed(&self) -> u64 {
        let mut n = 0;
        #[cfg(feature = "sensor-decibel")]
        {
            n += self.decibel_dma.frames();
        }
        #[cfg(feature = "sensor-pms5003")]
        {
            n += self.pms5003_dma.frames();
        }
        n
    }

    /// Total DMA frames lost to full rings, all engines.
    #[allow(unused_mut)] // a zero-feature build sums nothing
    pub fn overruns(&self) -> u64 {
        let mut n = 0;
        #[cfg(feature = "sensor-decibel")]
        {
            n += self.decibel_dma.overruns();
        }
        #[cfg(feature = "sensor-pms5003")]
        {
            n += self.pms5003_dma.overruns();
        }
        n
    }

    /// Total measurements emitted by sources + status heartbeat.
    #[allow(unused_mut)] // a zero-feature build sums nothing
    pub fn measurements(&self) -> u64 {
        let mut n = 0;
        #[cfg(feature = "sensor-decibel")]
        {
            n += self.decibel.emitted();
        }
        #[cfg(feature = "sensor-pms5003")]
        {
            n += self.pms5003.emitted();
        }
        #[cfg(feature = "sensor-pax")]
        {
            n += self.pax.emitted();
        }
        #[cfg(feature = "sensor-bme280")]
        {
            n += self.bme280.emitted();
        }
        #[cfg(feature = "sensor-sht4x")]
        {
            n += self.sht4x.emitted();
        }
        #[cfg(feature = "sensor-scd4x")]
        {
            n += self.scd4x.emitted();
        }
        #[cfg(feature = "sensor-bh1750")]
        {
            n += self.bh1750.emitted();
        }
        #[cfg(feature = "ops-status")]
        {
            n += self.status.emitted();
        }
        n
    }

    /// Total measurements drained by all compiled transports.
    #[allow(unused_mut)] // a zero-feature build sums nothing
    pub fn drained(&self) -> u64 {
        let mut n = 0;
        #[cfg(feature = "transport-http")]
        {
            n += self.http.sent();
        }
        #[cfg(feature = "transport-mqtt")]
        {
            n += self.mqtt.sent();
        }
        #[cfg(feature = "transport-mtls")]
        {
            n += self.mtls.sent();
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end with the default feature set: one DMA-fed sensor (decibel),
    /// one transport (http), and the status heartbeat. Conservation must hold
    /// exactly — which requires an exactly-known compiled set, so the test
    /// only runs when nothing else is compiled in (the flag matrix covers
    /// the other combinations).
    #[cfg(all(
        feature = "sensor-decibel",
        feature = "transport-http",
        feature = "ops-status",
        not(any(
            feature = "sensor-pax",
            feature = "sensor-bme280",
            feature = "sensor-sht4x",
            feature = "sensor-scd4x",
            feature = "sensor-pms5003",
            feature = "sensor-bh1750",
            feature = "transport-mqtt",
            feature = "transport-mtls",
            feature = "ota",
            feature = "provisioning",
            feature = "deep-sleep",
            feature = "logging-defmt"
        ))
    ))]
    #[test]
    fn end_to_end_conservation_default_path() {
        let mut g = Graph::build();
        let mut last = Tick::default();
        const STEPS: u64 = 200;
        for _ in 0..STEPS {
            last = g.step();
        }
        // The status heartbeat is a second, timer-driven source on this
        // path: it emits every PERIOD_STEPS steps and routes through the
        // same transport as the sensor traffic, so it is counted too.
        let heartbeats = STEPS / (ops::status::PERIOD_STEPS as u64);
        // Every window handed by the engine was folded and drained.
        assert_eq!(last.frames_handed, STEPS);
        assert_eq!(last.overruns, 0);
        assert_eq!(last.measurements, STEPS + heartbeats);
        assert_eq!(last.routed, STEPS + heartbeats);
        assert_eq!(last.drained, STEPS + heartbeats);
        // Nothing is left in flight.
        assert!(g.decibel.pcm.is_empty());
        assert!(g.decibel.out.is_empty());
        assert!(g.http.in_ring.is_empty());
    }

    /// Broadcast fan-out: with exactly the bh1750 + http + mqtt set
    /// compiled, every emission routes to each transport (routed counts
    /// once per transport). Exact totals need the exact set, so other
    /// features are excluded — the trio anchor in `just flag-matrix`
    /// compiles and tests precisely this combination.
    #[cfg(all(
        feature = "sensor-bh1750",
        feature = "transport-http",
        feature = "transport-mqtt",
        not(any(
            feature = "sensor-decibel",
            feature = "sensor-pax",
            feature = "sensor-bme280",
            feature = "sensor-sht4x",
            feature = "sensor-scd4x",
            feature = "sensor-pms5003",
            feature = "transport-mtls",
            feature = "ops-status",
            feature = "ota",
            feature = "provisioning",
            feature = "deep-sleep",
            feature = "logging-defmt"
        ))
    ))]
    #[test]
    fn broadcast_reaches_every_compiled_transport() {
        let mut g = Graph::build();
        let steps = 50;
        let mut last = Tick::default();
        for _ in 0..steps {
            last = g.step();
        }
        assert_eq!(last.measurements, steps);
        assert_eq!(last.routed, 2 * steps); // http + mqtt
        assert_eq!(last.drained, 2 * steps);
        assert_eq!(g.http.sent(), steps as u64);
        assert_eq!(g.mqtt.sent(), steps as u64);
        // Per-topic seq advanced once per emission on that topic.
        assert_eq!(g.mqtt.seq_of(&Measurement::Light { lux: 0 }), steps as u64);
    }

    /// The pump is deterministic: same steps, same totals.
    #[cfg(all(feature = "sensor-decibel", feature = "transport-http"))]
    #[test]
    fn the_pump_is_deterministic() {
        let mut a = Graph::build();
        let mut b = Graph::build();
        for _ in 0..64 {
            assert_eq!(a.step(), b.step());
        }
    }

    /// Latency: a measurement that enters a ring in step N is drained by
    /// the end of step N (the pump finishes with routing and drains).
    /// Same exact-default-set gate as the conservation test: one step's
    /// drain count is only exactly known when one source is compiled in.
    #[cfg(all(
        feature = "sensor-decibel",
        feature = "transport-http",
        feature = "ops-status",
        not(any(
            feature = "sensor-pax",
            feature = "sensor-bme280",
            feature = "sensor-sht4x",
            feature = "sensor-scd4x",
            feature = "sensor-pms5003",
            feature = "sensor-bh1750",
            feature = "transport-mqtt",
            feature = "transport-mtls",
            feature = "ota",
            feature = "provisioning",
            feature = "deep-sleep",
            feature = "logging-defmt"
        ))
    ))]
    #[test]
    fn end_to_end_latency_is_one_step() {
        let mut g = Graph::build();
        // Seed one window's worth of bytes without polling.
        let mut engine = SimulatedDma::new(sensors::decibel::WINDOW_BYTES);
        assert_eq!(engine.transfer(&g.decibel.pcm, 1), 1);
        // One step: fold → route → drain, all inside the step.
        let t = g.step();
        assert_eq!(t.drained, 1);
        assert!(g.http.in_ring.is_empty());
    }

    /// Ops-only graph (no sensors, no transports): ticks advance, nothing
    /// routes, nothing panics.
    #[cfg(all(
        feature = "ota",
        not(any(
            feature = "sensor-decibel",
            feature = "sensor-pms5003",
            feature = "sensor-pax",
            feature = "sensor-bme280",
            feature = "sensor-sht4x",
            feature = "sensor-scd4x",
            feature = "sensor-bh1750",
            feature = "ops-status"
        ))
    ))]
    #[test]
    fn ops_only_graph_advances() {
        let mut g = Graph::build();
        for _ in 0..10 {
            g.step();
        }
        assert_eq!(g.ota.ticks(), 10);
        let t = g.tick();
        assert_eq!(t.measurements, 0);
        assert_eq!(t.routed, 0);
        assert_eq!(t.drained, 0);
    }

    /// Graph footprint stays inside the fixed budget.
    #[test]
    fn graph_fits_the_budget() {
        assert!(Graph::BYTES <= crate::budget::BUDGET_BYTES);
    }
}
