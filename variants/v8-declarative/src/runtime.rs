//! The runtime half of the interpreter: executable pipelines built from the
//! validated graph.
//!
//! [`crate::graph`] turns declarations into validated [`Instance`]s — kinds
//! resolved, parameters checked. This module gives every instance its
//! *execution state*: a source that produces samples, a stage chain that
//! transforms them, and a sink that delivers them, all as closed enums over
//! the same statically-typed, feature-gated kinds the registry validates
//! (`build_source`/`build_stage`/`build_sink` mirror the registry's arms, so
//! a kind that is not compiled in has no runtime shape either). No trait
//! objects: the interpreter is a `match` over the kinds, which is what keeps
//! it small, `no_std`-friendly, and exhaustive under every feature world.
//!
//! Behavioural honesty — what runs today vs. what the driver PRs bring:
//!
//! - the plumbing kinds are real: `pipe` sources drain their upstream's
//!   finished stream, `aggregate`/`throttle` window and thin the stream,
//!   `clock` emits a contract-valid status heartbeat per beat;
//! - hardware sources are `Pending` — they produce no samples until their
//!   driver PR lands, so their pipelines idle (validated and scheduled, but
//!   nothing flows). The mechanism does not fake sensor readings;
//! - capability stages (`a-weight`, `c-weight`, `window-stats`,
//!   `pax-integrate`) are identity pass-throughs until their DSP/integration
//!   behaviour arrives with the driver PRs;
//! - sinks count deliveries and keep the last accepted payload; a
//!   measurement of another type is dropped and counted, never sent.
//!
//! The engine runs pipelines in topological order, so within one
//! [`crate::engine::Engine::tick`] an upstream always executes before the
//! `pipe` consumers that tap its stream.

#[cfg(feature = "ops-status")]
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::decl::{ParamGet, Params, PipelineDecl};
use crate::graph::Instance;
use crate::measurement::Measurement;
#[cfg(feature = "ops-status")]
use crate::measurement::{StatusReport, StatusResult};
// `registry::` paths and `MeasurementType` are only used by the HTTP/MQTT
// sink builders; in other feature worlds the bare kind names suffice.
#[cfg(any(feature = "transport-http", feature = "transport-mqtt"))]
use crate::registry::{self, MeasurementType};
use crate::registry::{SinkKind, SourceKind, StageKind};

/// How many finished measurements a pipeline may buffer for downstream
/// `pipe` consumers before the oldest is dropped. Bounds heap use when a
/// consumer is slow or absent; the loss is counted per pipeline
/// ([`crate::engine::Engine::outbox_dropped`]), never silent.
pub(crate) const OUTBOX_CAP: usize = 64;

/// A runnable source. One variant per compiled-in [`SourceKind`]; hardware
/// kinds are [`SourceExec::Pending`] until their driver PR lands.
#[derive(Clone, Debug)]
pub enum SourceExec {
    /// Consumes another pipeline's finished stream (`pipe`). The upstream's
    /// run index is resolved by the engine at instantiation.
    Pipe {
        /// Index into the engine's run list; always smaller than this run's
        /// own index (topological order).
        upstream: usize,
    },
    /// Wall-clock beats (`ops-status`): emits one contract-valid status
    /// heartbeat per beat. The beat counter gives every heartbeat a
    /// distinguishable payload.
    #[cfg(feature = "ops-status")]
    Clock { beat: u64 },
    /// A hardware source awaiting its driver PR: produces no samples.
    Pending,
}

/// A runnable stage. One variant per compiled-in [`StageKind`].
#[derive(Clone, Debug)]
pub enum StageExec {
    /// `aggregate`: holds the most recent sample and emits it once per
    /// resolved window — `window_s` converted into a beat count from the
    /// pipeline's cadence. (The decibel rollup's Leq math is the driver
    /// PR's; the windowing mechanism is the engine's.)
    Aggregate {
        /// Samples per window: `window_s·1000 / cadence_ms`, at least 1.
        beats_per_window: u64,
        seen: u64,
        held: Option<Measurement>,
    },
    /// `throttle`: passes every `every`-th sample (the first passes).
    Throttle { every: u64, seen: u64 },
    /// A capability stage whose behaviour arrives with its driver PR
    /// (`a-weight`, `c-weight`, `window-stats`, `pax-integrate`): a typed
    /// pass-through slot in the chain.
    Identity,
}

impl StageExec {
    /// Push one sample through the stage; `None` ends that sample's walk
    /// (a window that is not yet full, or a sample throttled out).
    pub fn apply(&mut self, m: Measurement) -> Option<Measurement> {
        match self {
            StageExec::Aggregate {
                beats_per_window,
                seen,
                held,
            } => {
                *seen += 1;
                *held = Some(m);
                if *seen >= *beats_per_window {
                    *seen = 0;
                    held.take()
                } else {
                    None
                }
            }
            StageExec::Throttle { every, seen } => {
                *seen += 1;
                if (*seen - 1) % *every == 0 {
                    Some(m)
                } else {
                    None
                }
            }
            StageExec::Identity => Some(m),
        }
    }
}

/// A runnable sink. One variant per compiled-in [`SinkKind`]; every sink
/// counts what it delivers, drops what it cannot send, and keeps the last
/// accepted payload for diagnostics.
#[derive(Clone, Debug)]
pub enum SinkExec {
    /// HTTP JSON ingest (`transport-http`): accepts measurements of its
    /// declared type. The encode-and-post arrives with the transport PR.
    #[cfg(feature = "transport-http")]
    Http {
        mtype: MeasurementType,
        delivered: u64,
        dropped: u64,
        last: Option<Measurement>,
    },
    /// MQTT v2 protobuf ingest (`transport-mqtt`): same accept rule.
    #[cfg(feature = "transport-mqtt")]
    Mqtt {
        mtype: MeasurementType,
        delivered: u64,
        dropped: u64,
        last: Option<Measurement>,
    },
    /// Retained status heartbeat (`ops-status`): accepts status reports.
    #[cfg(feature = "ops-status")]
    StatusHeartbeat {
        delivered: u64,
        dropped: u64,
        last: Option<Measurement>,
    },
    /// Diagnostic logging (`logging-defmt`): accepts everything.
    #[cfg(feature = "logging-defmt")]
    Log {
        delivered: u64,
        last: Option<Measurement>,
    },
}

impl SinkExec {
    /// Deliver one finished measurement. A measurement the sink cannot send
    /// (another type than declared) is dropped and counted — never a panic:
    /// the device loop must survive a mis-declared `pipe` edge.
    pub fn deliver(&mut self, m: &Measurement) {
        match self {
            #[cfg(feature = "transport-http")]
            SinkExec::Http {
                mtype,
                delivered,
                dropped,
                last,
            } => {
                if m.type_name() == mtype.name() {
                    *delivered += 1;
                    *last = Some(m.clone());
                } else {
                    *dropped += 1;
                }
            }
            #[cfg(feature = "transport-mqtt")]
            SinkExec::Mqtt {
                mtype,
                delivered,
                dropped,
                last,
            } => {
                if m.type_name() == mtype.name() {
                    *delivered += 1;
                    *last = Some(m.clone());
                } else {
                    *dropped += 1;
                }
            }
            #[cfg(feature = "ops-status")]
            SinkExec::StatusHeartbeat {
                delivered,
                dropped,
                last,
            } => {
                if matches!(m, Measurement::Status(_)) {
                    *delivered += 1;
                    *last = Some(m.clone());
                } else {
                    *dropped += 1;
                }
            }
            #[cfg(feature = "logging-defmt")]
            SinkExec::Log { delivered, last } => {
                *delivered += 1;
                *last = Some(m.clone());
            }
        }
    }

    /// Deliveries this sink accepted so far.
    pub fn delivered(&self) -> u64 {
        match *self {
            #[cfg(feature = "transport-http")]
            SinkExec::Http { delivered, .. } => delivered,
            #[cfg(feature = "transport-mqtt")]
            SinkExec::Mqtt { delivered, .. } => delivered,
            #[cfg(feature = "ops-status")]
            SinkExec::StatusHeartbeat { delivered, .. } => delivered,
            #[cfg(feature = "logging-defmt")]
            SinkExec::Log { delivered, .. } => delivered,
        }
    }

    /// Deliveries this sink dropped (a measurement of another type).
    pub fn dropped(&self) -> u64 {
        match *self {
            #[cfg(feature = "transport-http")]
            SinkExec::Http { dropped, .. } => dropped,
            #[cfg(feature = "transport-mqtt")]
            SinkExec::Mqtt { dropped, .. } => dropped,
            #[cfg(feature = "ops-status")]
            SinkExec::StatusHeartbeat { dropped, .. } => dropped,
            // The log sink accepts everything; it has no drop counter.
            #[cfg(feature = "logging-defmt")]
            SinkExec::Log { .. } => 0,
        }
    }

    /// The last measurement this sink accepted, for diagnostics.
    pub fn last(&self) -> Option<&Measurement> {
        match self {
            #[cfg(feature = "transport-http")]
            SinkExec::Http { last, .. } => last.as_ref(),
            #[cfg(feature = "transport-mqtt")]
            SinkExec::Mqtt { last, .. } => last.as_ref(),
            #[cfg(feature = "ops-status")]
            SinkExec::StatusHeartbeat { last, .. } => last.as_ref(),
            #[cfg(feature = "logging-defmt")]
            SinkExec::Log { last, .. } => last.as_ref(),
        }
    }
}

/// One runnable pipeline: execution state for a validated [`Instance`],
/// plus the scheduler fields the engine advances.
#[derive(Clone, Debug)]
pub struct PipelineRun {
    pub(crate) name: String,
    pub(crate) source: SourceExec,
    pub(crate) stages: Vec<StageExec>,
    pub(crate) sink: SinkExec,
    pub(crate) cadence_ms: u64,
    /// Resolved upstream run index for `pipe` sources.
    pub(crate) upstream: Option<usize>,
    /// Next due beat in ms; the engine advances it on every fire.
    pub(crate) next_due: u64,
    /// The pipeline's finished stream — every measurement that completed
    /// the stage chain, whether or not the sink accepted it. `pipe`
    /// consumers tap this; the sink is one terminal consumer, not a filter.
    pub(crate) outbox: Vec<Measurement>,
    pub(crate) outbox_dropped: u64,
}

impl PipelineRun {
    /// Build the execution state for a validated instance.
    ///
    /// `decl` must be the declaration the instance was validated from (same
    /// pipeline name, stages in the same order — `graph::validate` walks
    /// the declaration in order and mirrors it into the instance). All
    /// parameter `expect`s are re-reads of values the validator already
    /// enforced; the validator is the gate, the builder never re-decides.
    pub(crate) fn build(inst: &Instance, decl: &PipelineDecl) -> Self {
        PipelineRun {
            name: inst.name.clone(),
            source: build_source(inst.source),
            stages: inst
                .stages
                .iter()
                .zip(decl.stages.iter())
                .map(|(&kind, node)| build_stage(kind, &node.params, inst.cadence_ms))
                .collect(),
            sink: build_sink(inst.sink, &decl.sink.params),
            cadence_ms: inst.cadence_ms,
            upstream: None, // resolved by the engine once all runs exist
            next_due: 0,
            outbox: Vec::new(),
            outbox_dropped: 0,
        }
    }
}

/// The heartbeat a `clock` source emits per beat: a contract-valid status
/// report (non-empty subsystem, a `knownResults` value) with the beat count
/// as its message, so consumers can tell beats apart.
#[cfg(feature = "ops-status")]
pub(crate) fn clock_heartbeat(beat: u64) -> Measurement {
    let mut report = StatusReport::new("firmware", StatusResult::Success)
        .expect("non-empty subsystem");
    report.message = Some(format!("beat {beat}"));
    Measurement::status(report)
}

/// Build a source's execution state. Hardware kinds have no driver yet —
/// they all map to [`SourceExec::Pending`] and idle until their PR lands.
fn build_source(kind: SourceKind) -> SourceExec {
    match kind {
        SourceKind::Pipe => SourceExec::Pipe { upstream: 0 },
        #[cfg(feature = "ops-status")]
        SourceKind::Clock => SourceExec::Clock { beat: 0 },
        #[cfg(feature = "sensor-decibel")]
        SourceKind::Decibel => SourceExec::Pending,
        #[cfg(feature = "sensor-pax")]
        SourceKind::Pir => SourceExec::Pending,
        #[cfg(feature = "sensor-bme280")]
        SourceKind::Bme280 => SourceExec::Pending,
        #[cfg(feature = "sensor-sht4x")]
        SourceKind::Sht4x => SourceExec::Pending,
        #[cfg(feature = "sensor-scd4x")]
        SourceKind::Scd4x => SourceExec::Pending,
        #[cfg(feature = "sensor-pms5003")]
        SourceKind::Pms5003 => SourceExec::Pending,
        #[cfg(feature = "sensor-bh1750")]
        SourceKind::Bh1750 => SourceExec::Pending,
    }
}

/// Build a stage's execution state, resolving window parameters into beat
/// counts from the pipeline's own cadence.
fn build_stage(kind: StageKind, params: &Params, cadence_ms: u64) -> StageExec {
    match kind {
        StageKind::Aggregate => {
            let window_s = params.u64_param("window_s").expect("validated positive");
            // Window smaller than one beat ⇒ emit every beat.
            let beats_per_window = (window_s.saturating_mul(1000) / cadence_ms).max(1);
            StageExec::Aggregate {
                beats_per_window,
                seen: 0,
                held: None,
            }
        }
        StageKind::Throttle => {
            let every = params.u64_param("every").expect("validated positive");
            StageExec::Throttle { every, seen: 0 }
        }
        // Capability slots: typed pass-throughs until the driver PRs land.
        #[cfg(feature = "sensor-decibel")]
        StageKind::AWeight | StageKind::CWeight | StageKind::WindowStats => {
            StageExec::Identity
        }
        #[cfg(feature = "sensor-pax")]
        StageKind::PaxIntegrate => StageExec::Identity,
    }
}

/// Build a sink's execution state, carrying the declared measurement type
/// as the accept rule.
fn build_sink(kind: SinkKind, params: &Params) -> SinkExec {
    // Without an HTTP/MQTT transport no sink kind reads parameters at
    // build time (status/log sinks carry none), leaving `params` unused.
    #[cfg(not(any(feature = "transport-http", feature = "transport-mqtt")))]
    let _ = params;
    match kind {
        #[cfg(feature = "transport-http")]
        SinkKind::HttpIngest => {
            let mtype = registry::req_measurement(params, "type").expect("validated");
            SinkExec::Http {
                mtype,
                delivered: 0,
                dropped: 0,
                last: None,
            }
        }
        #[cfg(feature = "transport-mqtt")]
        SinkKind::MqttIngest => {
            let mtype = registry::req_measurement(params, "type").expect("validated");
            SinkExec::Mqtt {
                mtype,
                delivered: 0,
                dropped: 0,
                last: None,
            }
        }
        #[cfg(feature = "ops-status")]
        SinkKind::StatusHeartbeat => SinkExec::StatusHeartbeat {
            delivered: 0,
            dropped: 0,
            last: None,
        },
        #[cfg(feature = "logging-defmt")]
        SinkKind::LogSink => SinkExec::Log {
            delivered: 0,
            last: None,
        },
    }
}

#[cfg(test)]
// Execution-state unit tests that need no kind-specific features (the
// throttle stage is plumbing; `light` measurements are base-model).
// Kind-shaped behaviour is exercised end-to-end by engine::tests.
mod tests {
    use super::*;

    #[test]
    fn throttle_passes_the_first_sample_then_every_nth_thereafter() {
        // Sample-count semantics: no wall clock involved — the stage cannot
        // drift from the engine's scheduler, it simply counts what it sees.
        let mut throttle = StageExec::Throttle { every: 3, seen: 0 };
        let sample = || Measurement::light(42.0).expect("finite lux");
        assert!(throttle.apply(sample()).is_some(), "sample 1 passes");
        assert!(throttle.apply(sample()).is_none(), "sample 2 suppressed");
        assert!(throttle.apply(sample()).is_none(), "sample 3 suppressed");
        assert!(throttle.apply(sample()).is_some(), "sample 4 passes");
        assert!(throttle.apply(sample()).is_none(), "sample 5 suppressed");
    }

    #[test]
    fn throttle_every_one_passes_every_sample() {
        let mut throttle = StageExec::Throttle { every: 1, seen: 0 };
        for i in 0..10 {
            let m = Measurement::light(1.0).expect("finite lux");
            assert!(throttle.apply(m).is_some(), "every=1 passes sample {i}");
        }
    }
}
