//! The boot-time interpreter: declarations → validated instances → ticks.
//!
//! This is V8's runtime core. At boot, [`Engine::instantiate`] parses a
//! pipelines document, validates it through [`crate::graph`], and holds the
//! resulting instances in topological (dependency) order. From then on the
//! device loop only calls [`Engine::tick`]: it returns the pipelines whose
//! cadence came due, in schedule order. Real stage behaviour arrives with
//! the driver PRs — the scaffold's job is to prove the schedule, the
//! gating, and the failure modes, not to fake DSP.

use alloc::vec::Vec;

use crate::decl::Pipelines;
use crate::graph::{self, GraphError, Instance};

/// The instantiated pipeline engine.
#[derive(Clone, Debug)]
pub struct Engine {
    instances: Vec<Instance>,
}

impl Engine {
    /// Parse, validate, and instantiate a pipelines document. A `pipelines.toml`
    /// that names kinds this binary does not contain (feature flags off),
    /// references an undeclared pipeline, contains a cycle, or violates the
    /// cadence rules fails here — at boot, loudly.
    pub fn instantiate(pipelines: &Pipelines) -> Result<Self, GraphError> {
        Ok(Self {
            instances: graph::validate(pipelines)?,
        })
    }

    /// Number of compiled pipelines in dependency order.
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Whether the engine runs no pipelines (an empty document is a parse
    /// error, so a valid engine always has at least one — but the accessor
    /// keeps the invariant explicit for tests).
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// The schedule in dependency order: `(name, cadence_ms)` pairs.
    pub fn schedule(&self) -> Vec<(&str, u64)> {
        self.instances
            .iter()
            .map(|i| (i.name.as_str(), i.cadence_ms))
            .collect()
    }

    /// Advance the scheduler to `now_ms`. Returns the pipelines due at or
    /// before `now`, in dependency order; each due pipeline's next beat is
    /// rescheduled one or more cadence steps past `now` on its original
    /// grid, so slow consumers catch up without bursting.
    pub fn tick(&mut self, now_ms: u64) -> Vec<&str> {
        let mut due = Vec::new();
        for inst in &mut self.instances {
            if inst.next_due <= now_ms {
                due.push(inst.name.as_str());
                let missed = (now_ms - inst.next_due) / inst.cadence_ms + 1;
                inst.next_due += missed * inst.cadence_ms;
            }
        }
        due
    }
}

#[cfg(test)]
// Tests are gated per the kinds they name; the trio-gated ones mirror the
// default feature set (see the note in graph::tests).
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
        // (sensor-decibel, transport-http, ops-status): a decibel chain, its
        // hourly pipe-fed rollup, and the status heartbeat.
        let doc = Pipelines::parse(include_str!("../config/pipelines.toml"))
            .expect("committed pipelines.toml parses");
        let engine =
            Engine::instantiate(&doc).expect("committed pipelines.toml is valid");
        let schedule = engine.schedule();
        assert!(engine.len() >= 2);
        // Every pipe-sourced pipeline comes after its upstream.
        for (pos, (name, _)) in schedule.iter().enumerate() {
            if *name == "decibel-hourly" {
                let upstream = schedule[..pos].iter().any(|(n, _)| *n == "decibel-leq");
                assert!(
                    upstream,
                    "decibel-leq must be scheduled before decibel-hourly"
                );
            }
        }
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
}
