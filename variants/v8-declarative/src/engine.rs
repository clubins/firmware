//! The boot-time interpreter: declarations → validated instances → runnable
//! pipelines → ticks.
//!
//! This is V8's runtime core. At boot, [`Engine::instantiate`] parses a
//! pipelines document, validates it through [`crate::graph`], builds each
//! instance's execution state ([`crate::runtime`]), and wires the `pipe`
//! edges between pipelines. From then on the device loop only calls
//! [`Engine::tick`]: it executes the pipelines whose cadence came due —
//! each run pulls its upstream's finished stream, produces its own samples,
//! pushes them through the stage chain, and delivers to its sink — and
//! returns the runs that fired, in schedule order.
//!
//! Real hardware behaviour arrives with the driver PRs: the BME280 and
//! SHT4x sources already run their datasheet flows on the engine's I²C bus
//! (one flow leg per beat — see [`crate::sensors`]); the remaining sensor
//! sources are `Pending` (their pipelines idle) and capability stages are
//! identity slots. Sinks count deliveries. The mechanism — graph
//! validation, flag-gated instantiation, stream wiring, cadence execution
//! — is complete and host-tested; each new driver slots in without
//! touching the engine.

use alloc::vec::Vec;

#[cfg(any(feature = "sensor-bme280", feature = "sensor-sht4x"))]
use crate::bus::SimulatedI2c;
use crate::decl::Pipelines;
use crate::graph::{self, GraphError};
use crate::measurement::Measurement;
use crate::runtime::{PipelineRun, SourceExec};
#[cfg(any(feature = "sensor-bme280", feature = "sensor-sht4x"))]
use crate::sensors::BeatDriver;

/// The instantiated pipeline engine.
#[derive(Clone, Debug)]
pub struct Engine {
    /// Runs in topological (dependency) order: an upstream's index is
    /// always smaller than its consumers'.
    runs: Vec<PipelineRun>,
    /// The engine's I²C bus — the one mechanism instance the declared
    /// sensor sources' drivers share, one flow leg per beat. The scripted
    /// simulated bus stands in for the ESP32-S3's I²C peripheral until the
    /// device glue PR lands (same transaction trait; the swap is a field
    /// assignment — see [`Engine::with_bus`]).
    #[cfg(any(feature = "sensor-bme280", feature = "sensor-sht4x"))]
    bus: SimulatedI2c,
}

impl Engine {
    /// Parse, validate, and instantiate a pipelines document. A `pipelines.toml`
    /// that names kinds this binary does not contain (feature flags off),
    /// references an undeclared pipeline, contains a cycle, or violates the
    /// cadence rules fails here — at boot, loudly.
    pub fn instantiate(pipelines: &Pipelines) -> Result<Self, GraphError> {
        let validated = graph::validate(pipelines)?;

        // Execution state per instance; the declaration supplies the
        // parameters the validator already checked (same order — validate
        // mirrors the declaration into the instance).
        let mut runs: Vec<PipelineRun> = validated
            .iter()
            .map(|inst| {
                let decl = pipelines
                    .pipelines
                    .iter()
                    .find(|d| d.name == inst.name)
                    .expect("validate returns the declared pipelines");
                PipelineRun::build(inst, decl)
            })
            .collect();

        // Resolve `pipe` sources to run indexes. Topological order
        // guarantees the upstream sorts first; assert it so a future change
        // to the ordering cannot silently break the split_at_mut below.
        for (i, inst) in validated.iter().enumerate() {
            if let Some(upstream) = &inst.upstream {
                let idx = runs
                    .iter()
                    .position(|r| r.name.as_str() == upstream.as_str())
                    .expect("references were validated");
                debug_assert!(idx < i, "topo order must place upstreams first");
                runs[i].upstream = Some(idx);
            }
        }

        Ok(Self {
            runs,
            #[cfg(any(feature = "sensor-bme280", feature = "sensor-sht4x"))]
            bus: SimulatedI2c::new(),
        })
    }

    /// Swap in a bus. Host tests script register flows on the simulated
    /// bus; the device build keeps the constructor default and wires the
    /// real peripheral behind the same transaction trait when its glue
    /// PR lands.
    #[cfg(any(feature = "sensor-bme280", feature = "sensor-sht4x"))]
    pub fn with_bus(mut self, bus: SimulatedI2c) -> Self {
        self.bus = bus;
        self
    }

    /// Number of compiled pipelines in dependency order.
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether the engine runs no pipelines (an empty document is a parse
    /// error, so a valid engine always has at least one — but the accessor
    /// keeps the invariant explicit for tests).
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// The schedule in dependency order: `(name, cadence_ms)` pairs.
    pub fn schedule(&self) -> Vec<(&str, u64)> {
        self.runs
            .iter()
            .map(|i| (i.name.as_str(), i.cadence_ms))
            .collect()
    }

    /// Deliveries a pipeline's sink accepted, by name.
    pub fn delivered(&self, name: &str) -> Option<u64> {
        self.runs
            .iter()
            .find(|r| r.name.as_str() == name)
            .map(|r| r.sink.delivered())
    }

    /// Measurements a pipeline's sink dropped (another type than declared),
    /// by name.
    pub fn dropped(&self, name: &str) -> Option<u64> {
        self.runs
            .iter()
            .find(|r| r.name.as_str() == name)
            .map(|r| r.sink.dropped())
    }

    /// Stream samples a pipeline dropped at its outbox cap (its consumers
    /// were too slow), by name.
    pub fn outbox_dropped(&self, name: &str) -> Option<u64> {
        self.runs
            .iter()
            .find(|r| r.name.as_str() == name)
            .map(|r| r.outbox_dropped)
    }

    /// Bus-flow faults a pipeline's source has counted since instantiation
    /// (the driver re-enters bring-up on its next beat), by name. A pipeline
    /// emitting nothing while this climbs is a dead or absent sensor, not a
    /// silent one. Sources without a bus never fault.
    #[cfg(any(feature = "sensor-bme280", feature = "sensor-sht4x"))]
    pub fn source_faults(&self, name: &str) -> Option<u64> {
        self.runs
            .iter()
            .find(|r| r.name.as_str() == name)
            .map(|r| r.source.faults())
    }

    /// The last measurement a pipeline's sink accepted, by name.
    pub fn last_delivered(&self, name: &str) -> Option<&Measurement> {
        self.runs
            .iter()
            .find(|r| r.name.as_str() == name)
            .and_then(|r| r.sink.last())
    }

    /// Advance the scheduler to `now_ms` and execute what came due. Returns
    /// the pipelines that fired, in dependency order; each due pipeline's
    /// next beat is rescheduled one or more cadence steps past `now` on its
    /// original grid, so slow consumers catch up without bursting.
    pub fn tick(&mut self, now_ms: u64) -> Vec<&str> {
        // First pass: collect due indexes and advance their grids (index
        // bookkeeping keeps the borrow checker happy and the order stable).
        let mut due: Vec<usize> = Vec::new();
        for (i, run) in self.runs.iter_mut().enumerate() {
            if run.next_due <= now_ms {
                due.push(i);
                let missed = (now_ms - run.next_due) / run.cadence_ms + 1;
                run.next_due += missed * run.cadence_ms;
            }
        }
        // Second pass: execute in dependency order, so a `pipe` consumer
        // sees everything its upstream finished up to now.
        for &i in &due {
            self.execute(i);
        }
        due.iter().map(|&i| self.runs[i].name.as_str()).collect()
    }

    /// Execute one pipeline run: gather inputs (the upstream's finished
    /// stream, then this beat's own samples), run the stage chain, deliver
    /// to the sink, and append what finished to this pipeline's stream.
    fn execute(&mut self, i: usize) {
        // Topological order puts the upstream first, so the split is safe.
        let (earlier, later) = self.runs.split_at_mut(i);
        let run = &mut later[0];

        let mut inputs: Vec<Measurement> = Vec::new();
        if let Some(up) = run.upstream {
            // Drain whatever the upstream finished since our last beat —
            // a slow consumer catches up on the whole backlog, one sample
            // at a time through its own chain.
            inputs.extend(core::mem::take(&mut earlier[up].outbox));
        }
        match &mut run.source {
            // The upstream's stream is this run's input; nothing extra.
            SourceExec::Pipe { .. } => {}
            // Hardware sources have no driver yet: they produce nothing.
            SourceExec::Pending => {}
            #[cfg(feature = "ops-status")]
            SourceExec::Clock { beat } => {
                *beat += 1;
                inputs.push(crate::runtime::clock_heartbeat(*beat));
            }
            // Sensor drivers: one datasheet-flow leg per beat. A fault is
            // counted, not propagated — the source stays scheduled and the
            // driver re-enters bring-up on its next beat.
            #[cfg(feature = "sensor-bme280")]
            SourceExec::Bme280 { driver, faults } => {
                if let Some(m) = Self::sensor_beat(&mut self.bus, driver, faults) {
                    inputs.push(m);
                }
            }
        }

        for sample in inputs {
            // The stage chain: a stage returning None ends that sample's
            // walk (window not yet full, or throttled out).
            let mut current = Some(sample);
            for stage in &mut run.stages {
                match current.take() {
                    Some(m) => current = stage.apply(m),
                    None => break,
                }
            }
            if let Some(m) = current {
                // Sinks are terminal consumers of the stream; the outbox is
                // what `pipe` consumers tap — capped, drops counted.
                run.sink.deliver(&m);
                run.outbox.push(m);
                if run.outbox.len() > crate::runtime::OUTBOX_CAP {
                    run.outbox.remove(0);
                    run.outbox_dropped += 1;
                }
            }
        }
    }

    /// Drive one sensor's datasheet flow by one beat on the engine's bus
    /// and surface the measurement it produced, if any. A fault (bus error,
    /// wrong id, bad frame) is counted on the run, not propagated: a flaky
    /// sensor retries, a dead one keeps its fault count climbing.
    #[cfg(any(feature = "sensor-bme280", feature = "sensor-sht4x"))]
    fn sensor_beat<D: BeatDriver>(
        bus: &mut SimulatedI2c,
        driver: &mut D,
        faults: &mut u64,
    ) -> Option<Measurement> {
        match driver.beat(bus) {
            Ok(Some(m)) => Some(m),
            Ok(None) => None,
            // The fault's kind stays with the driver (its tests pin it);
            // the count is the engine's signal.
            Err(_) => {
                *faults += 1;
                None
            }
        }
    }
}

#[cfg(test)]
// Kind coverage follows the same per-test gating note as graph::tests:
// tests naming clock/http kinds are gated to the default feature set.
mod tests {
    use super::*;

    /// Parse + instantiate; panic with the display error if invalid.
    fn engine(src: &str) -> Engine {
        let doc = Pipelines::parse(src).expect("test document parses");
        Engine::instantiate(&doc).expect("test document is valid")
    }

    #[test]
    #[cfg(all(
        feature = "sensor-decibel",
        feature = "transport-http",
        feature = "ops-status"
    ))]
    fn instantiates_the_default_document_in_dependency_order() {
        // The committed config instantiates under the default feature set
        // (sensor-decibel, transport-http, ops-status): a decibel chain, the
        // clock-fed status heartbeat, and the decibel chain's hourly pipe-fed
        // rollup.
        let doc = Pipelines::parse(include_str!("../config/pipelines.toml"))
            .expect("committed pipelines.toml parses");
        let engine =
            Engine::instantiate(&doc).expect("committed pipelines.toml is valid");
        let schedule = engine.schedule();
        assert!(engine.len() >= 3);
        let leq_pos = schedule
            .iter()
            .position(|(n, _)| *n == "decibel-leq")
            .expect("decibel-leq in the default document");
        let hourly_pos = schedule
            .iter()
            .position(|(n, _)| *n == "decibel-hourly")
            .expect("decibel-hourly in the default document");
        assert!(
            leq_pos < hourly_pos,
            "an upstream must precede its pipe consumers"
        );
        // Freshly built runs: nothing executed yet, nothing dropped.
        assert_eq!(engine.delivered("status"), Some(0));
        assert_eq!(engine.delivered("decibel-leq"), Some(0));
        assert_eq!(engine.outbox_dropped("decibel-leq"), Some(0));
    }

    #[test]
    #[cfg(all(feature = "sensor-decibel", feature = "transport-http"))]
    fn cadence_resolution_fires_pipelines_on_their_beats() {
        let mut engine = engine(
            r#"
            [[pipeline]]
            name = "fast"
            source = { kind = "pipe", pipeline = "base", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "slow"
            source = { kind = "pipe", pipeline = "base", cadence_ms = 2000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "base"
            source = { kind = "decibel", cadence_ms = 500 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );

        // Dependency order first: base (500) → fast (1000) → slow (2000).
        assert_eq!(
            engine.schedule(),
            [("base", 500), ("fast", 1000), ("slow", 2000)]
        );

        assert_eq!(engine.tick(0), ["base", "fast", "slow"]);
        assert_eq!(engine.tick(499), Vec::<&str>::new());
        assert_eq!(engine.tick(500), ["base"]);
        assert_eq!(engine.tick(999), Vec::<&str>::new());
        assert_eq!(engine.tick(1000), ["base", "fast"]);
        assert_eq!(engine.tick(1500), ["base"]);
        assert_eq!(engine.tick(2000), ["base", "fast", "slow"]);
        assert_eq!(engine.tick(2001), Vec::<&str>::new());
    }

    #[test]
    #[cfg(all(feature = "sensor-decibel", feature = "transport-http"))]
    fn slow_consumers_catch_up_without_bursting() {
        let mut engine = engine(
            r#"
            [[pipeline]]
            name = "base"
            source = { kind = "decibel", cadence_ms = 100 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "hourly"
            source = { kind = "pipe", pipeline = "base", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert_eq!(engine.tick(0), ["base", "hourly"]);
        // Jump far past several beats: the engine reports each pipeline once
        // and reschedules on its original cadence grid, strictly in the future.
        assert_eq!(engine.tick(10_000), ["base", "hourly"]);
        // base's grid: 100 ms steps from 10_100; hourly's: 1000 ms from 11_000.
        assert_eq!(engine.tick(10_050), Vec::<&str>::new());
        assert_eq!(engine.tick(10_099), Vec::<&str>::new());
        assert_eq!(engine.tick(10_100), ["base"]);
        assert_eq!(engine.tick(11_000), ["base", "hourly"]);
    }

    #[test]
    #[cfg(not(feature = "sensor-bh1750"))]
    fn a_config_edit_cannot_enable_an_uncompiled_kind() {
        // sensor-bh1750 is not in the default set: a pipeline that names it
        // must fail instantiation even though the TOML is well-formed.
        let doc = Pipelines::parse(
            r#"
            [[pipeline]]
            name = "lux"
            source = { kind = "bh1750", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "light"
            "#,
        )
        .expect("parses");
        let err = Engine::instantiate(&doc).expect_err("kind is not compiled in");
        assert!(
            matches!(err, GraphError::UnknownSourceKind { kind, .. } if kind == "bh1750")
        );
    }

    #[test]
    #[cfg(all(feature = "sensor-decibel", feature = "transport-http"))]
    fn flag_gated_kind_instantiates_when_compiled() {
        // The mirror image, under the default set: the decibel kind exists,
        // so the same shape of pipeline instantiates cleanly.
        let doc = Pipelines::parse(
            r#"
            [[pipeline]]
            name = "db"
            source = { kind = "decibel", cadence_ms = 1000 }
            [[pipeline.stages]]
            kind = "a-weight"
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        )
        .expect("parses");
        let engine = Engine::instantiate(&doc).expect("decibel is compiled in");
        assert_eq!(engine.schedule(), [("db", 1000)]);
    }

    #[test]
    fn mtls_gate_is_enforced_through_validation() {
        // tls defaults to true: an mqtt-ingest sink without the
        // transport-mtls flag compiled in cannot validate. In worlds where
        // transport-mqtt itself is off, the sink kind is unknown instead.
        // The pipe source exists in every feature world, so the sink verdict
        // is the first error in all of them; "base" exists so the all-on
        // world passes reference and cadence checks and validates cleanly.
        let doc = Pipelines::parse(
            r#"
            [[pipeline]]
            name = "db"
            source = { kind = "pipe", pipeline = "base", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "mqtt-ingest"
            type = "decibel"

            [[pipeline]]
            name = "base"
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        )
        .expect("parses");
        let result = Engine::instantiate(&doc);
        if cfg!(feature = "transport-mqtt") {
            if cfg!(feature = "transport-mtls") {
                result.expect("mTLS compiled in");
            } else {
                assert!(matches!(result, Err(GraphError::BadParam { .. })));
            }
        } else {
            assert!(matches!(
                result,
                Err(GraphError::UnknownSinkKind { kind, .. }) if kind == "mqtt-ingest"
            ));
        }
    }

    #[test]
    #[cfg(feature = "transport-http")]
    fn cyclic_declarations_fail_the_boot() {
        let doc = Pipelines::parse(
            r#"
            [[pipeline]]
            name = "a"
            source = { kind = "pipe", pipeline = "b", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "b"
            source = { kind = "pipe", pipeline = "a", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        )
        .expect("parses");
        assert!(matches!(
            Engine::instantiate(&doc),
            Err(GraphError::Cycle(_))
        ));
    }

    // ---- execution (runtime) behaviour, default feature world ----

    #[test]
    #[cfg(all(feature = "ops-status", feature = "transport-http"))]
    fn clock_pipeline_delivers_heartbeats_and_counts_them() {
        let mut engine = engine(
            r#"
            [[pipeline]]
            name = "hb"
            source = { kind = "clock", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "status-heartbeat"
            "#,
        );
        assert_eq!(engine.tick(0), ["hb"]); // beat 1
        assert_eq!(engine.tick(999), Vec::<&str>::new());
        assert_eq!(engine.tick(1000), ["hb"]); // beat 2
        assert_eq!(engine.delivered("hb"), Some(2));
        assert_eq!(engine.dropped("hb"), Some(0));
        match engine.last_delivered("hb") {
            Some(Measurement::Status(report)) => {
                assert_eq!(report.message.as_deref(), Some("beat 2"))
            }
            other => panic!("expected the second heartbeat, got {other:?}"),
        }
    }

    #[test]
    #[cfg(all(feature = "ops-status", feature = "transport-http"))]
    fn pipe_consumer_drains_its_upstreams_stream() {
        let mut engine = engine(
            r#"
            [[pipeline]]
            name = "producer"
            source = { kind = "clock", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "status-heartbeat"

            [[pipeline]]
            name = "consumer"
            source = { kind = "pipe", pipeline = "producer", cadence_ms = 2000 }
            [pipeline.sink]
            kind = "status-heartbeat"
            "#,
        );
        // The consumer accepts status (heartbeat) — `status` is deliberately
        // not a measurement type in the registry, so `http-ingest` cannot
        // declare it and the retained status sink is the status destination.
        // Beat 0: producer emits "beat 1", consumer drains it in the same
        // tick (dependency order), so both sinks see one delivery.
        assert_eq!(engine.tick(0), ["producer", "consumer"]);
        assert_eq!(engine.delivered("producer"), Some(1));
        assert_eq!(engine.delivered("consumer"), Some(1));
        // Producer fires alone at 1000 ms: "beat 2" waits in its stream.
        assert_eq!(engine.tick(1000), ["producer"]);
        assert_eq!(engine.delivered("producer"), Some(2));
        assert_eq!(engine.delivered("consumer"), Some(1));
        // Consumer's beat: the backlog ("beat 2" from 1000 ms plus "beat 3"
        // just produced) drains in one run, in order.
        assert_eq!(engine.tick(2000), ["producer", "consumer"]);
        assert_eq!(engine.delivered("consumer"), Some(3));
        assert_eq!(engine.outbox_dropped("producer"), Some(0));
        match engine.last_delivered("consumer") {
            Some(Measurement::Status(report)) => {
                assert_eq!(report.message.as_deref(), Some("beat 3"))
            }
            other => panic!("expected the drained heartbeat, got {other:?}"),
        }
    }

    #[test]
    #[cfg(all(feature = "ops-status", feature = "transport-http"))]
    fn aggregate_stage_gates_until_the_window_is_full() {
        let mut engine = engine(
            r#"
            [[pipeline]]
            name = "agg"
            source = { kind = "clock", cadence_ms = 1000 }
            [[pipeline.stages]]
            kind = "aggregate"
            window_s = 3
            [pipeline.sink]
            kind = "status-heartbeat"
            "#,
        );
        // window_s 3 at a 1000 ms cadence → one emission per 3 beats: the
        // stage holds the window's latest sample and releases it at the
        // boundary.
        assert_eq!(engine.tick(0), ["agg"]); // 1/3
        assert_eq!(engine.tick(1000), ["agg"]); // 2/3
        assert_eq!(engine.delivered("agg"), Some(0));
        assert_eq!(engine.tick(2000), ["agg"]); // 3/3 → one sample out
        assert_eq!(engine.delivered("agg"), Some(1));
        assert_eq!(engine.tick(3000), ["agg"]); // 1/3
        assert_eq!(engine.delivered("agg"), Some(1));
        assert_eq!(engine.tick(4000), ["agg"]); // 2/3
        assert_eq!(engine.tick(5000), ["agg"]); // 3/3 → second out
        assert_eq!(engine.delivered("agg"), Some(2));
    }

    #[test]
    #[cfg(all(feature = "ops-status", feature = "transport-http"))]
    fn throttle_stage_passes_one_sample_per_interval() {
        let mut engine = engine(
            r#"
            [[pipeline]]
            name = "thr"
            source = { kind = "clock", cadence_ms = 1000 }
            [[pipeline.stages]]
            kind = "throttle"
            every = 10
            [pipeline.sink]
            kind = "status-heartbeat"
            "#,
        );
        // The throttle counts samples, not wall time: the first passes, the
        // next nine are suppressed, then the eleventh passes.
        for ms in (0..=9_000).step_by(1000) {
            assert_eq!(engine.tick(ms), ["thr"]);
        }
        assert_eq!(engine.delivered("thr"), Some(1));
        assert_eq!(engine.tick(10_000), ["thr"]); // sample 11 passes
        assert_eq!(engine.delivered("thr"), Some(2));
        assert_eq!(engine.tick(11_000), ["thr"]); // suppressed again
        assert_eq!(engine.delivered("thr"), Some(2));
    }

    #[test]
    #[cfg(all(feature = "ops-status", feature = "transport-http"))]
    fn sink_type_guard_drops_other_types_and_counts_them() {
        let mut engine = engine(
            r#"
            [[pipeline]]
            name = "guarded"
            source = { kind = "clock", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "weather"
            "#,
        );
        // Heartbeats are status measurements; a weather-declared sink must
        // never accept one — and must never do so silently.
        assert_eq!(engine.tick(0), ["guarded"]);
        assert_eq!(engine.tick(1000), ["guarded"]);
        assert_eq!(engine.delivered("guarded"), Some(0));
        assert_eq!(engine.dropped("guarded"), Some(2));
        assert!(engine.last_delivered("guarded").is_none());
    }

    #[test]
    #[cfg(all(feature = "ops-status", feature = "transport-http"))]
    fn outbox_cap_drops_the_oldest_samples_and_counts_them() {
        let mut engine = engine(
            r#"
            [[pipeline]]
            name = "solo"
            source = { kind = "clock", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "status-heartbeat"
            "#,
        );
        // 71 beats with no pipe consumer: the newest 64 stay buffered, the
        // oldest 7 are dropped — counted, never silent.
        for ms in (0..=70_000).step_by(1000) {
            engine.tick(ms);
        }
        assert_eq!(engine.outbox_dropped("solo"), Some(7));
    }

    #[test]
    #[cfg(all(feature = "sensor-decibel", feature = "transport-http"))]
    fn pending_hardware_source_produces_nothing() {
        // The decibel driver PR has not landed: the pipeline schedules and
        // executes, but its source is Pending — no samples, no deliveries,
        // no invented data.
        let mut engine = engine(
            r#"
            [[pipeline]]
            name = "db"
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        for ms in (0..=5000).step_by(1000) {
            assert_eq!(engine.tick(ms), ["db"]);
        }
        assert_eq!(engine.delivered("db"), Some(0));
        assert_eq!(engine.dropped("db"), Some(0));
        assert!(engine.last_delivered("db").is_none());
    }
}
