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
//! Real hardware behaviour arrives with the driver PRs: hardware sources
//! are `Pending` (their pipelines idle), capability stages are identity
//! slots, and sinks count deliveries. The mechanism — graph validation,
//! flag-gated instantiation, stream wiring, cadence execution — is complete
//! and host-tested; the drivers slot into it without touching the engine.

use alloc::vec::Vec;

use crate::decl::Pipelines;
use crate::graph::{self, GraphError};
use crate::measurement::Measurement;
use crate::runtime::{PipelineRun, SourceExec};

/// The instantiated pipeline engine.
#[derive(Clone, Debug)]
pub struct Engine {
    /// Runs in topological (dependency) order: an upstream's index is
    /// always smaller than its consumers'.
    runs: Vec<PipelineRun>,
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

        Ok(Self { runs })
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
}
