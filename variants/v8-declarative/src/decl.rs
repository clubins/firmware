//! Typed pipeline declarations, parsed from `config/pipelines.toml`.
//!
//! The declarative half of V8: a pipeline is *data* — a source node, an
//! ordered list of stage nodes, and a sink node, each a `kind` name plus
//! scalar parameters. This module turns the TOML text into those typed
//! declarations; it deliberately knows nothing about which kinds exist
//! (that is [`crate::registry`]'s job) or how they connect (that is
//! [`crate::graph`]'s job).
//!
//! The TOML layer is the `toml` crate's serde-free parser
//! (`default-features = false, features = ["parse"]`), so the whole module
//! is `no_std`-compatible and host-tested. Parameter values must be scalars
//! (string, integer, float, bool) — nested tables/arrays as parameters are
//! rejected with a pointed error, because a flat schema keeps the registry's
//! typed lookups honest and keeps the file editable by non-programmers.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use toml::de::{DeTable, DeValue};

/// A scalar parameter value in a pipeline declaration.
#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// The parameters of one node: every key except `kind` (sources also pull
/// `cadence_ms` out as a first-class field in [`crate::graph`]).
pub type Params = BTreeMap<String, Scalar>;

/// One node in a pipeline: a source, a stage, or a sink.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeDecl {
    pub kind: String,
    pub params: Params,
}

/// One declared pipeline: `source → stage* → sink`.
#[derive(Clone, Debug, PartialEq)]
pub struct PipelineDecl {
    pub name: String,
    pub source: NodeDecl,
    pub stages: Vec<NodeDecl>,
    pub sink: NodeDecl,
}

/// The whole parsed `pipelines.toml` document.
#[derive(Clone, Debug, PartialEq)]
pub struct Pipelines {
    pub pipelines: Vec<PipelineDecl>,
}

/// A failure while turning TOML text into typed declarations.
///
/// Variants are structured so tests can match them precisely, and every
/// variant names the location (`ctx`) it failed at — a venue operator
/// editing `pipelines.toml` should never have to guess.
#[derive(Debug, PartialEq)]
pub enum DeclError {
    /// The text is not valid TOML.
    Syntax(String),
    /// The document has no `[[pipeline]]` entries.
    NoPipelines,
    /// A root key other than `pipeline` was declared (typo guard — a
    /// misspelled `[[pipeline]]` must not silently parse to zero pipelines).
    UnknownRootKey(String),
    /// A pipeline entry or node was not a table.
    WrongShape { ctx: String, why: String },
    /// A required key is absent.
    MissingField { ctx: String, field: &'static str },
    /// A key was present but has the wrong value type.
    BadType {
        ctx: String,
        field: String,
        want: &'static str,
    },
    /// A key was present with the right shape but an unusable value.
    BadValue {
        ctx: String,
        field: String,
        why: String,
    },
}

impl core::fmt::Display for DeclError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DeclError::Syntax(e) => write!(f, "pipelines.toml is not valid TOML: {e}"),
            DeclError::NoPipelines => {
                write!(f, "pipelines.toml declares no [[pipeline]] entries")
            }
            DeclError::UnknownRootKey(k) => {
                write!(
                    f,
                    "pipelines.toml has unknown root key '{k}' (expected [[pipeline]])"
                )
            }
            DeclError::WrongShape { ctx, why } => write!(f, "{ctx}: {why}"),
            DeclError::MissingField { ctx, field } => {
                write!(f, "{ctx}: missing '{field}'")
            }
            DeclError::BadType { ctx, field, want } => {
                write!(f, "{ctx}: '{field}' must be {want}")
            }
            DeclError::BadValue { ctx, field, why } => {
                write!(f, "{ctx}: '{field}' {why}")
            }
        }
    }
}

impl Pipelines {
    /// Parse a pipelines document from TOML text.
    pub fn parse(src: &str) -> Result<Self, DeclError> {
        let spanned =
            DeTable::parse(src).map_err(|e| DeclError::Syntax(e.to_string()))?;
        let mut doc = spanned.into_inner();
        doc.make_owned();

        for key in doc.keys() {
            if key.get_ref().as_ref() != "pipeline" {
                return Err(DeclError::UnknownRootKey(key.get_ref().to_string()));
            }
        }

        let Some(entries) = doc.get("pipeline") else {
            return Err(DeclError::NoPipelines);
        };
        let DeValue::Array(list) = entries.get_ref() else {
            return Err(DeclError::WrongShape {
                ctx: "[[pipeline]]".into(),
                why: "expected an array of tables ([[pipeline]])".into(),
            });
        };

        let mut pipelines = Vec::with_capacity(list.len());
        for (i, entry) in list.iter().enumerate() {
            let ctx = alloc::format!("pipeline #{i}");
            let table = match entry.get_ref() {
                DeValue::Table(t) => t,
                _ => {
                    return Err(DeclError::WrongShape {
                        ctx,
                        why: "entry is not a table".into(),
                    });
                }
            };
            pipelines.push(parse_pipeline(table, &ctx)?);
        }

        if pipelines.is_empty() {
            return Err(DeclError::NoPipelines);
        }
        Ok(Pipelines { pipelines })
    }
}

/// Parse one `[[pipeline]]` entry.
fn parse_pipeline(table: &DeTable<'_>, ctx: &str) -> Result<PipelineDecl, DeclError> {
    let name = req_str(table, ctx, "name")?;
    let ctx = alloc::format!("pipeline '{name}'");

    let source_table = req_table(table, &ctx, "source")?;
    let source = parse_node(source_table, &alloc::format!("{ctx} source"))?;

    let mut stages = Vec::new();
    if let Some(stages_val) = table.get("stages") {
        let DeValue::Array(list) = stages_val.get_ref() else {
            return Err(DeclError::WrongShape {
                ctx: alloc::format!("{ctx} stages"),
                why: "expected an array of tables ([[pipeline.stages]])".into(),
            });
        };
        for (i, entry) in list.iter().enumerate() {
            let stage_table = match entry.get_ref() {
                DeValue::Table(t) => t,
                _ => {
                    return Err(DeclError::WrongShape {
                        ctx: alloc::format!("{ctx} stage #{i}"),
                        why: "entry is not a table".into(),
                    });
                }
            };
            stages.push(parse_node(
                stage_table,
                &alloc::format!("{ctx} stage #{i}"),
            )?);
        }
    }

    let sink_table = req_table(table, &ctx, "sink")?;
    let sink = parse_node(sink_table, &alloc::format!("{ctx} sink"))?;

    Ok(PipelineDecl {
        name,
        source,
        stages,
        sink,
    })
}

/// Parse a source/stage/sink node: `kind` plus scalar parameters.
fn parse_node(table: &DeTable<'_>, ctx: &str) -> Result<NodeDecl, DeclError> {
    let kind = req_str(table, ctx, "kind")?;
    let mut params = Params::new();
    for (key, value) in table.iter() {
        let field = key.get_ref().as_ref();
        if field == "kind" {
            continue;
        }
        params.insert(field.into(), scalar(value.get_ref(), ctx, field)?);
    }
    Ok(NodeDecl { kind, params })
}

/// Extract a scalar parameter value; nested structures are rejected.
fn scalar(value: &DeValue<'_>, ctx: &str, field: &str) -> Result<Scalar, DeclError> {
    match value {
        DeValue::String(s) => Ok(Scalar::Str(s.as_ref().into())),
        DeValue::Integer(i) => {
            let parsed = i64::from_str_radix(i.as_str(), i.radix()).map_err(|_| {
                DeclError::BadValue {
                    ctx: ctx.into(),
                    field: field.into(),
                    why: "is not a representable integer".into(),
                }
            })?;
            Ok(Scalar::Int(parsed))
        }
        DeValue::Float(fl) => {
            let parsed: f64 = fl.as_str().parse().map_err(|_| DeclError::BadValue {
                ctx: ctx.into(),
                field: field.into(),
                why: "is not a representable float".into(),
            })?;
            Ok(Scalar::Float(parsed))
        }
        DeValue::Boolean(b) => Ok(Scalar::Bool(*b)),
        DeValue::Datetime(_) => Err(DeclError::BadValue {
            ctx: ctx.into(),
            field: field.into(),
            why: "is a datetime — only scalar parameters are supported".into(),
        }),
        DeValue::Array(_) | DeValue::Table(_) => Err(DeclError::BadValue {
            ctx: ctx.into(),
            field: field.into(),
            why: "must be a scalar (string, integer, float, or bool)".into(),
        }),
    }
}

/// Required string field.
fn req_str(
    table: &DeTable<'_>,
    ctx: &str,
    field: &'static str,
) -> Result<String, DeclError> {
    match table.get(field) {
        None => Err(DeclError::MissingField {
            ctx: ctx.into(),
            field,
        }),
        Some(v) => match v.get_ref().as_str() {
            Some(s) => Ok(s.into()),
            None => Err(DeclError::BadType {
                ctx: ctx.into(),
                field: field.into(),
                want: "a string",
            }),
        },
    }
}

/// Required sub-table field.
fn req_table<'a, 'i>(
    table: &'a DeTable<'i>,
    ctx: &str,
    field: &'static str,
) -> Result<&'a DeTable<'i>, DeclError> {
    match table.get(field) {
        None => Err(DeclError::MissingField {
            ctx: ctx.into(),
            field,
        }),
        Some(v) => match v.get_ref().as_table() {
            Some(t) => Ok(t),
            None => Err(DeclError::BadType {
                ctx: ctx.into(),
                field: field.into(),
                want: "a table",
            }),
        },
    }
}

/// Typed lookups over parsed parameters, used by the registry to turn
/// `Params` into concrete stage configuration.
pub trait ParamGet {
    fn str_param(&self, key: &str) -> Result<String, ParamError>;
    fn u64_param(&self, key: &str) -> Result<u64, ParamError>;
    fn f64_param(&self, key: &str) -> Result<f64, ParamError>;
    fn bool_param(&self, key: &str) -> Result<bool, ParamError>;
    fn f64_param_or(&self, key: &str, default: f64) -> Result<f64, ParamError>;
    fn bool_param_or(&self, key: &str, default: bool) -> Result<bool, ParamError>;
}

/// A parameter lookup failure (the registry wraps these in
/// [`crate::graph::GraphError::MissingParam`] / `BadParam` with pipeline
/// context added).
#[derive(Debug, PartialEq)]
pub enum ParamError {
    Missing,
    BadType {
        want: &'static str,
    },
    /// Right type, unusable value (e.g. a negative `window_ms`).
    BadValue,
}

impl ParamGet for Params {
    fn str_param(&self, key: &str) -> Result<String, ParamError> {
        match self.get(key) {
            None => Err(ParamError::Missing),
            Some(Scalar::Str(s)) => Ok(s.clone()),
            Some(_) => Err(ParamError::BadType { want: "a string" }),
        }
    }

    fn u64_param(&self, key: &str) -> Result<u64, ParamError> {
        match self.get(key) {
            None => Err(ParamError::Missing),
            Some(Scalar::Int(i)) if *i >= 0 => Ok(*i as u64),
            Some(Scalar::Int(_)) => Err(ParamError::BadValue),
            Some(_) => Err(ParamError::BadType { want: "an integer" }),
        }
    }

    fn f64_param(&self, key: &str) -> Result<f64, ParamError> {
        match self.get(key) {
            None => Err(ParamError::Missing),
            Some(Scalar::Int(i)) => Ok(*i as f64),
            Some(Scalar::Float(f)) if f.is_finite() => Ok(*f),
            Some(Scalar::Float(_)) => Err(ParamError::BadValue),
            Some(_) => Err(ParamError::BadType { want: "a number" }),
        }
    }

    fn bool_param(&self, key: &str) -> Result<bool, ParamError> {
        match self.get(key) {
            None => Err(ParamError::Missing),
            Some(Scalar::Bool(b)) => Ok(*b),
            Some(_) => Err(ParamError::BadType { want: "a boolean" }),
        }
    }

    /// Optional float: missing → `default`, present → validated.
    fn f64_param_or(&self, key: &str, default: f64) -> Result<f64, ParamError> {
        match self.get(key) {
            None => Ok(default),
            Some(_) => self.f64_param(key),
        }
    }

    /// Optional bool: missing → `default`, present → validated.
    fn bool_param_or(&self, key: &str, default: bool) -> Result<bool, ParamError> {
        match self.get(key) {
            None => Ok(default),
            Some(_) => self.bool_param(key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_pipeline() {
        let doc = Pipelines::parse(
            r#"
            [[pipeline]]
            name = "decibel-leq"
            source = { kind = "decibel", cadence_ms = 1000 }
            [[pipeline.stages]]
            kind = "a-weight"
            [[pipeline.stages]]
            kind = "window-stats"
            window_ms = 125
            overload_threshold_db = 120.5
            log = true
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        )
        .expect("parses");
        assert_eq!(doc.pipelines.len(), 1);
        let p = &doc.pipelines[0];
        assert_eq!(p.name, "decibel-leq");
        assert_eq!(p.source.kind, "decibel");
        assert_eq!(p.source.params.u64_param("cadence_ms").ok(), Some(1000));
        assert_eq!(p.stages.len(), 2);
        assert_eq!(p.stages[0].kind, "a-weight");
        assert_eq!(p.stages[1].kind, "window-stats");
        assert_eq!(
            p.stages[1].params.f64_param("overload_threshold_db").ok(),
            Some(120.5)
        );
        assert_eq!(p.stages[1].params.bool_param("log").ok(), Some(true));
        assert_eq!(p.sink.kind, "http-ingest");
        assert_eq!(
            p.sink.params.str_param("type").ok().as_deref(),
            Some("decibel")
        );
    }

    #[test]
    fn stages_are_optional() {
        let doc = Pipelines::parse(
            r#"
            [[pipeline]]
            name = "raw"
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            type = "decibel"
            "#,
        )
        .expect("parses");
        assert_eq!(doc.pipelines[0].stages.len(), 0);
    }

    #[test]
    fn empty_document_is_rejected() {
        assert_eq!(Pipelines::parse(""), Err(DeclError::NoPipelines));
        assert_eq!(
            Pipelines::parse("# only a comment"),
            Err(DeclError::NoPipelines)
        );
    }

    #[test]
    fn unknown_root_key_is_a_typo_guard() {
        // `[[pipelines]]` (plural) would otherwise parse to zero pipelines.
        let err = Pipelines::parse("[[pipelins]]").expect_err("typo must fail");
        assert_eq!(err, DeclError::UnknownRootKey("pipelins".into()));
    }

    #[test]
    fn invalid_toml_is_a_syntax_error() {
        assert!(matches!(
            Pipelines::parse("[[pipeline"),
            Err(DeclError::Syntax(_))
        ));
    }

    #[test]
    fn missing_fields_name_their_context() {
        let err = Pipelines::parse(
            r#"
            [[pipeline]]
            source = { kind = "decibel", cadence_ms = 1000 }
            [pipeline.sink]
            kind = "http-ingest"
            "#,
        )
        .expect_err("missing name must fail");
        assert_eq!(
            err,
            DeclError::MissingField {
                ctx: "pipeline #0".into(),
                field: "name"
            }
        );
    }

    #[test]
    fn non_scalar_params_are_rejected() {
        let err = Pipelines::parse(
            r#"
            [[pipeline]]
            name = "x"
            source = { kind = "decibel", cadence_ms = 1000, weights = [1, 2] }
            [pipeline.sink]
            kind = "http-ingest"
            "#,
        )
        .expect_err("array param must fail");
        assert!(matches!(err, DeclError::BadValue { field, .. } if field == "weights"));
    }

    #[test]
    fn param_getters_type_check() {
        let mut params = Params::new();
        params.insert("n".into(), Scalar::Int(7));
        params.insert("s".into(), Scalar::Str("x".into()));
        params.insert("f".into(), Scalar::Float(1.5));
        params.insert("neg".into(), Scalar::Int(-3));

        assert_eq!(params.u64_param("n").ok(), Some(7));
        assert_eq!(params.u64_param("neg"), Err(ParamError::BadValue));
        assert_eq!(params.str_param("s").ok().as_deref(), Some("x"));
        assert_eq!(params.f64_param("n").ok(), Some(7.0)); // int widens
        assert_eq!(
            params.f64_param("s"),
            Err(ParamError::BadType { want: "a number" })
        );
        assert_eq!(params.u64_param("absent"), Err(ParamError::Missing));
        assert_eq!(params.f64_param_or("absent", 2.0).ok(), Some(2.0));
    }
}
