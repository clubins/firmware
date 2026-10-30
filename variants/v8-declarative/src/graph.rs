//! Boot-time validation of the pipeline declaration set.
//!
//! The interpreter's gatekeeper: every declared pipeline is checked here
//! before any instance is built, so a bad `pipelines.toml` fails loudly at
//! boot instead of misbehaving in the field. Validation runs in one pass:
//!
//! 1. pipeline names are unique;
//! 2. every node kind resolves through the feature-gated registry — a kind
//!    whose flag is not compiled in is as unknown as one that never existed;
//! 3. every node's parameters type-check against its kind (typos included);
//! 4. `pipe` sources name pipelines that exist;
//! 5. the pipeline graph is acyclic;
//! 6. a `pipe`-sourced pipeline's cadence is a nonzero multiple of its
//!    upstream's — you cannot consume samples faster than they are produced.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::decl::{ParamError, ParamGet, Pipelines};
use crate::registry::{self, Kind, ParamIssue};

/// Why a declaration set cannot be instantiated.
#[derive(Clone, Debug, PartialEq)]
pub enum GraphError {
    DuplicatePipeline(String),
    UnknownSourceKind {
        pipeline: String,
        kind: String,
    },
    UnknownStageKind {
        pipeline: String,
        kind: String,
    },
    UnknownSinkKind {
        pipeline: String,
        kind: String,
    },
    MissingParam {
        pipeline: String,
        node: String,
        field: &'static str,
    },
    BadParam {
        pipeline: String,
        node: String,
        why: String,
    },
    UnknownPipelineRef {
        pipeline: String,
        refers: String,
    },
    /// Pipeline names along the cycle, in walk order.
    Cycle(Vec<String>),
    BadCadence {
        pipeline: String,
        why: String,
    },
}

impl core::fmt::Display for GraphError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GraphError::DuplicatePipeline(name) => {
                write!(f, "pipeline '{name}' is declared twice")
            }
            GraphError::UnknownSourceKind { pipeline, kind } => write!(
                f,
                "pipeline '{pipeline}': source kind '{kind}' is not compiled in"
            ),
            GraphError::UnknownStageKind { pipeline, kind } => write!(
                f,
                "pipeline '{pipeline}': stage kind '{kind}' is not compiled in"
            ),
            GraphError::UnknownSinkKind { pipeline, kind } => write!(
                f,
                "pipeline '{pipeline}': sink kind '{kind}' is not compiled in"
            ),
            GraphError::MissingParam {
                pipeline,
                node,
                field,
            } => write!(f, "pipeline '{pipeline}': {node} is missing '{field}'"),
            GraphError::BadParam {
                pipeline,
                node,
                why,
            } => write!(f, "pipeline '{pipeline}': {node} {why}"),
            GraphError::UnknownPipelineRef { pipeline, refers } => write!(
                f,
                "pipeline '{pipeline}' sources from '{refers}', which is not declared"
            ),
            GraphError::Cycle(path) => {
                write!(f, "pipeline cycle: {}", path.join(" → "))
            }
            GraphError::BadCadence { pipeline, why } => {
                write!(f, "pipeline '{pipeline}': {why}")
            }
        }
    }
}

/// One validated pipeline instance: kinds resolved, parameters checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub name: String,
    pub source: registry::SourceKind,
    pub stages: Vec<registry::StageKind>,
    pub sink: registry::SinkKind,
    /// Sampling cadence in ms (> 0). For `pipe` sources this is the
    /// consumption cadence, a multiple of the upstream's.
    pub cadence_ms: u64,
    /// Upstream pipeline name for `pipe` sources.
    pub upstream: Option<String>,
    /// Scheduler bookkeeping: the next due beat in ms. Starts at 0; the
    /// engine owns advancing it (see `engine::Engine::tick`).
    pub(crate) next_due: u64,
}

/// Validate a whole declaration set and return instances in topological
/// (dependency) order — the order the interpreter must instantiate them in.
pub fn validate(pipelines: &Pipelines) -> Result<Vec<Instance>, GraphError> {
    check_unique_names(pipelines)?;

    let mut instances = Vec::with_capacity(pipelines.pipelines.len());
    for decl in &pipelines.pipelines {
        instances.push(validate_pipeline(decl)?);
    }

    check_references(&instances)?;
    let order = topo_sort(&instances)?;
    let mut ordered: Vec<Instance> = Vec::with_capacity(instances.len());
    for name in order {
        let idx = instances
            .iter()
            .position(|i| i.name == name)
            .expect("topo_sort only returns declared names");
        ordered.push(instances.swap_remove(idx));
    }

    check_cadence_chains(&ordered)?;
    Ok(ordered)
}

/// Names must be unique — they are the graph's node identities.
fn check_unique_names(pipelines: &Pipelines) -> Result<(), GraphError> {
    let mut seen = BTreeSet::new();
    for decl in &pipelines.pipelines {
        if !seen.insert(decl.name.as_str()) {
            return Err(GraphError::DuplicatePipeline(decl.name.clone()));
        }
    }
    Ok(())
}

/// Validate one pipeline: kind resolution + parameter typing.
fn validate_pipeline(decl: &crate::decl::PipelineDecl) -> Result<Instance, GraphError> {
    let name = decl.name.clone();

    // Source: resolve kind, validate params, extract cadence + upstream.
    let source_kind = registry::lookup_source(&decl.source.kind).ok_or_else(|| {
        GraphError::UnknownSourceKind {
            pipeline: name.clone(),
            kind: decl.source.kind.clone(),
        }
    })?;
    validate_params(&name, "source", source_kind.validate(&decl.source.params))?;
    let cadence_ms = cadence_of(&name, &decl.source.params)?;
    let upstream = match source_kind {
        registry::SourceKind::Pipe => Some(
            decl.source
                .params
                .str_param("pipeline")
                .expect("Pipe validate requires 'pipeline'"),
        ),
        _ => None,
    };

    // Stages: resolve and validate in declared order.
    let mut stages = Vec::with_capacity(decl.stages.len());
    for (i, stage) in decl.stages.iter().enumerate() {
        let kind = registry::lookup_stage(&stage.kind).ok_or_else(|| {
            GraphError::UnknownStageKind {
                pipeline: name.clone(),
                kind: stage.kind.clone(),
            }
        })?;
        let node = alloc::format!("stage #{i} ({})", kind.name());
        validate_params(&name, &node, kind.validate(&stage.params))?;
        stages.push(kind);
    }

    // Sink: resolve and validate.
    let sink_kind = registry::lookup_sink(&decl.sink.kind).ok_or_else(|| {
        GraphError::UnknownSinkKind {
            pipeline: name.clone(),
            kind: decl.sink.kind.clone(),
        }
    })?;
    validate_params(&name, "sink", sink_kind.validate(&decl.sink.params))?;

    Ok(Instance {
        name,
        source: source_kind,
        stages,
        sink: sink_kind,
        cadence_ms,
        upstream,
        next_due: 0,
    })
}

/// Add pipeline context to a kind's parameter verdict.
fn validate_params(
    pipeline: &str,
    node: &str,
    result: Result<(), ParamIssue>,
) -> Result<(), GraphError> {
    result.map_err(|issue| {
        let node = node.to_string();
        match issue {
            ParamIssue::Missing { field } => GraphError::MissingParam {
                pipeline: pipeline.into(),
                node,
                field,
            },
            ParamIssue::BadType { field, want } => GraphError::BadParam {
                pipeline: pipeline.into(),
                node,
                why: alloc::format!("parameter '{field}' must be {want}"),
            },
            ParamIssue::BadValue { field, why } => GraphError::BadParam {
                pipeline: pipeline.into(),
                node,
                why: alloc::format!("parameter '{field}' {why}"),
            },
            ParamIssue::Unknown { field } => GraphError::BadParam {
                pipeline: pipeline.into(),
                node,
                why: alloc::format!("has unknown parameter '{field}'"),
            },
        }
    })
}

/// Sources must declare a positive integer `cadence_ms`.
fn cadence_of(pipeline: &str, params: &crate::decl::Params) -> Result<u64, GraphError> {
    match params.u64_param("cadence_ms") {
        Err(ParamError::Missing) => Err(GraphError::MissingParam {
            pipeline: pipeline.into(),
            node: "source".into(),
            field: "cadence_ms",
        }),
        Err(ParamError::BadType { want }) => Err(GraphError::BadParam {
            pipeline: pipeline.into(),
            node: "source".into(),
            why: alloc::format!("parameter 'cadence_ms' must be {want}"),
        }),
        Err(ParamError::BadValue) => Err(GraphError::BadCadence {
            pipeline: pipeline.into(),
            why: "cadence_ms must be a non-negative integer".into(),
        }),
        Ok(0) => Err(GraphError::BadCadence {
            pipeline: pipeline.into(),
            why: "cadence_ms must be positive (0 would spin forever)".into(),
        }),
        Ok(cadence) => Ok(cadence),
    }
}

/// `pipe` sources must name a declared pipeline.
fn check_references(instances: &[Instance]) -> Result<(), GraphError> {
    for inst in instances {
        if let Some(upstream) = &inst.upstream
            && !instances.iter().any(|i| &i.name == upstream)
        {
            return Err(GraphError::UnknownPipelineRef {
                pipeline: inst.name.clone(),
                refers: upstream.clone(),
            });
        }
    }
    Ok(())
}

/// Kahn topological sort over `pipe` edges. Deterministic: ties broken by
/// declaration order, so the returned order is stable across boots.
fn topo_sort(instances: &[Instance]) -> Result<Vec<String>, GraphError> {
    use alloc::collections::BTreeMap;

    // Edge A → B when B sources from A.
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
    for inst in instances {
        in_degree.entry(inst.name.as_str()).or_insert(0);
    }
    for inst in instances {
        if let Some(upstream) = &inst.upstream {
            // Reference existence was checked before this runs.
            dependents
                .entry(upstream.as_str())
                .or_default()
                .push(inst.name.as_str());
            *in_degree.entry(inst.name.as_str()).or_insert(0) += 1;
        }
    }

    // Seed with roots in declaration order.
    let mut ready: Vec<&str> = instances
        .iter()
        .filter(|i| in_degree[i.name.as_str()] == 0)
        .map(|i| i.name.as_str())
        .collect();
    let mut order = Vec::with_capacity(instances.len());

    while let Some(name) = ready.first().copied() {
        ready.remove(0);
        order.push(name.into());
        if let Some(children) = dependents.get(name) {
            for child in children {
                let deg = in_degree.get_mut(child).expect("child is declared");
                *deg -= 1;
                if *deg == 0 {
                    ready.push(child);
                }
            }
        }
    }

    if order.len() != instances.len() {
        // Anything not scheduled is on a cycle; walk it out for the report.
        let stuck: Vec<&str> = instances
            .iter()
            .map(|i| i.name.as_str())
            .filter(|n| !order.contains(&n.to_string()))
            .collect();
        return Err(GraphError::Cycle(cycle_path(instances, &stuck)));
    }
    Ok(order)
}

/// Reconstruct one cycle path starting from the first stuck node.
fn cycle_path(instances: &[Instance], stuck: &[&str]) -> Vec<String> {
    let mut path: Vec<String> = Vec::new();
    let mut current: String = stuck[0].into();
    let mut guard = 0;
    while guard <= instances.len() + 1 {
        guard += 1;
        path.push(current.clone());
        let upstream = instances
            .iter()
            .find(|i| i.name == current)
            .and_then(|i| i.upstream.clone());
        match upstream {
            Some(next) if stuck.contains(&next.as_str()) && !path.contains(&next) => {
                current = next;
            }
            Some(next) if path.contains(&next) => {
                // Close the loop visually: rotate so the cycle starts cleanly.
                let pos = path.iter().position(|n| *n == next).expect("checked");
                return path.split_off(pos);
            }
            _ => return path,
        }
    }
    path
}

/// `pipe` cadences must be nonzero multiples of their upstream's.
fn check_cadence_chains(ordered: &[Instance]) -> Result<(), GraphError> {
    for inst in ordered {
        let Some(upstream_name) = &inst.upstream else {
            continue;
        };
        let upstream = ordered
            .iter()
            .find(|i| &i.name == upstream_name)
            .expect("refs validated before ordering");
        if inst.cadence_ms < upstream.cadence_ms
            || inst.cadence_ms % upstream.cadence_ms != 0
        {
            return Err(GraphError::BadCadence {
                pipeline: inst.name.clone(),
                why: alloc::format!(
                    "cadence_ms = {} is not a multiple of upstream '{}' cadence_ms = {}",
                    inst.cadence_ms,
                    upstream.name,
                    upstream.cadence_ms
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
// Every test here builds pipelines from the default-set kinds (decibel or
// clock sources, http-ingest sinks), so the module is gated on the trio.
// The all-off world has no root source kind at all — pipe/aggregate/throttle
// cannot form a valid graph without one — so there is nothing to assert
// there; per-kind presence is proven in registry::tests.
#[cfg(all(
    feature = "sensor-decibel",
    feature = "transport-http",
    feature = "ops-status"
))]
mod tests {
    use super::*;
    use crate::decl::Pipelines;

    /// Parse a document; panic with the display error if invalid.
    fn parse(src: &str) -> Pipelines {
        Pipelines::parse(src).expect("test document parses")
    }

    #[test]
    fn validates_a_clean_graph_in_dependency_order() {
        let doc = parse(
            r#"
            [[pipeline]]
            name = "raw"
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "rolled"
            source = { kind = "pipe", pipeline = "raw", cadence_ms = 60000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        let order = validate(&doc).expect("valid");
        let names: Vec<&str> = order.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["raw", "rolled"]);
        assert_eq!(order[1].upstream.as_deref(), Some("raw"));
        assert_eq!(order[1].cadence_ms, 60_000);
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let doc = parse(
            r#"
            [[pipeline]]
            name = "x"
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "x"
            source = { kind = "clock", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "status-heartbeat"
            "#,
        );
        assert_eq!(
            validate(&doc),
            Err(GraphError::DuplicatePipeline("x".into()))
        );
    }

    #[test]
    fn unknown_source_kind_is_rejected() {
        let doc = parse(
            r#"
            [[pipeline]]
            name = "x"
            source = { kind = "thermal", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert_eq!(
            validate(&doc),
            Err(GraphError::UnknownSourceKind {
                pipeline: "x".into(),
                kind: "thermal".into()
            })
        );
    }

    #[test]
    fn unknown_stage_and_sink_kinds_are_rejected() {
        let base = r#"
            [[pipeline]]
            name = "x"
            source = { kind = "decibel", cadence_ms = 1000 }
            [[pipeline.stages]]
            kind = "vibrato"
            [pipeline.sink]
            kind = "carrier-pigeon"
            "#;
        let doc = parse(base);
        assert!(matches!(
            validate(&doc),
            Err(GraphError::UnknownStageKind { kind, .. }) if kind == "vibrato"
        ));

        let doc = parse(
            r#"
            [[pipeline]]
            name = "x"
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "carrier-pigeon"
            "#,
        );
        assert!(matches!(
            validate(&doc),
            Err(GraphError::UnknownSinkKind { kind, .. }) if kind == "carrier-pigeon"
        ));
    }

    #[test]
    fn missing_required_params_are_named() {
        let doc = parse(
            r#"
            [[pipeline]]
            name = "x"
            source = { kind = "decibel", cadence_ms = 1000 }
            [[pipeline.stages]]
            kind = "window-stats"
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert!(matches!(
            validate(&doc),
            Err(GraphError::MissingParam { node, field, .. })
                if node.contains("window-stats") && field == "window_ms"
        ));
    }

    #[test]
    fn unknown_params_are_treated_as_typos() {
        // A typo'd structural key hits the kind's unknown-key check first,
        // which names the exact misspelling — the precise boot diagnosis.
        let doc = parse(
            r#"
            [[pipeline]]
            name = "x"
            source = { kind = "decibel", cadance_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert!(matches!(
            validate(&doc),
            Err(GraphError::BadParam { why, .. }) if why.contains("cadance_ms")
        ));
    }

    #[test]
    fn missing_cadence_is_a_param_error() {
        let doc = parse(
            r#"
            [[pipeline]]
            name = "x"
            source = { kind = "decibel" }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert!(matches!(
            validate(&doc),
            Err(GraphError::MissingParam { field, .. }) if field == "cadence_ms"
        ));
    }

    #[test]
    fn zero_cadence_is_rejected() {
        let doc = parse(
            r#"
            [[pipeline]]
            name = "x"
            source = { kind = "decibel", cadence_ms = 0 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert!(matches!(
            validate(&doc),
            Err(GraphError::BadCadence { why, .. }) if why.contains("positive")
        ));
    }

    #[test]
    fn unknown_pipe_reference_is_rejected() {
        let doc = parse(
            r#"
            [[pipeline]]
            name = "x"
            source = { kind = "pipe", pipeline = "ghost", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert_eq!(
            validate(&doc),
            Err(GraphError::UnknownPipelineRef {
                pipeline: "x".into(),
                refers: "ghost".into()
            })
        );
    }

    #[test]
    fn cycles_are_detected() {
        let doc = parse(
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
        );
        assert!(
            matches!(validate(&doc), Err(GraphError::Cycle(path)) if path.len() == 2)
        );
    }

    #[test]
    fn self_reference_is_a_cycle() {
        let doc = parse(
            r#"
            [[pipeline]]
            name = "a"
            source = { kind = "pipe", pipeline = "a", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert!(matches!(validate(&doc), Err(GraphError::Cycle(_))));
    }

    #[test]
    fn downstream_cadence_must_be_a_multiple_of_upstream() {
        let bad_multiple = parse(
            r#"
            [[pipeline]]
            name = "raw"
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "down"
            source = { kind = "pipe", pipeline = "raw", cadence_ms = 2500 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert!(matches!(
            validate(&bad_multiple),
            Err(GraphError::BadCadence { pipeline, why })
                if pipeline == "down" && why.contains("2500")
        ));

        let faster = parse(
            r#"
            [[pipeline]]
            name = "raw"
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "down"
            source = { kind = "pipe", pipeline = "raw", cadence_ms = 500 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        assert!(matches!(
            validate(&faster),
            Err(GraphError::BadCadence { .. })
        ));
    }

    #[test]
    fn diamond_graphs_sort_without_cycles() {
        // raw → left, raw → right, both → joined. All multiples hold.
        let doc = parse(
            r#"
            [[pipeline]]
            name = "raw"
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "left"
            source = { kind = "pipe", pipeline = "raw", cadence_ms = 2000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"

            [[pipeline]]
            name = "joined"
            source = { kind = "pipe", pipeline = "left", cadence_ms = 4000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        );
        let order = validate(&doc).expect("diamond is a DAG");
        let names: Vec<&str> = order.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names.len(), 3);
        assert_eq!(names[0], "raw");
    }
}
