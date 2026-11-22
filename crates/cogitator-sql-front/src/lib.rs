//! Builds HIR from dbt SQL+Jinja (specs/ir-pipeline/01). The only crate
//! in the pipeline that sees sqlparser or `minijinja::machinery`.
//!
//! Stages, each in its own module: [`mask`] (Jinja regions, same-length
//! mask), [`segment`] (tokens and clause-position Jinja), [`caps`] (the
//! input limits), [`parse`] (statement + per-part recovery), [`lower`]
//! (AST to bound HIR). Compiled-SQL mapping arrives in `compiled/`.
//!
//! Span policy: `ColRef` spans, `Ident` spans, `Lit` and `Jinja` expr
//! spans and FROM base spans are exact. Every other span is derived from
//! its children and approximate (`ExprKind::span_is_exact`); it may
//! highlight but must not drive an edit.

use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::common::{HoleKind, ModelConfig};
use cogitator_ir::hir::{verify_hir, Body, Hir, Query, Scope, ScopeKind};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::schema::ProjectSchemas;
use cogitator_ir::{Degradation, DegradationKind, Lowered, Span};

mod caps;
pub mod compiled;
mod config;
mod dialect;
mod jinja;
mod lower;
mod mask;
mod parse;
mod segment;
mod types;

pub use types::parse_sql_type;

/// Folds an unquoted name from schema metadata the way the binder folds
/// identifiers for `dialect`, so names from YAML, a catalog or a manifest
/// compare equal to names written in SQL.
pub fn fold_schema_name<'a>(name: &'a str, dialect: &Dialect) -> std::borrow::Cow<'a, str> {
    dialect::DialectRules::for_dialect(dialect).fold_name(name)
}

use mask::{scan, Scanned, Stop};
use segment::Lexed;

/// One model's text and the facts the front end needs beside it.
#[derive(Debug, Clone)]
pub struct HirInput<'a> {
    /// The text spans will index: the live template or compiled SQL.
    pub text: &'a str,
    pub dialect: &'a Dialect,
    pub origin: SourceOrigin,
    pub config: ModelConfig,
}

/// Name-resolution context for relations outside the model.
#[derive(Debug, Clone, Default)]
pub struct SchemaEnv {
    pub schemas: Arc<ProjectSchemas>,
    /// The model being built, for `{{ this }}` and self references.
    pub model: Option<Arc<str>>,
}

/// Builds and binds the HIR for `input`. Never fails: gaps surface as
/// `Degradation`s or a `Declined` result carrying guidance.
///
/// A template whose only control flow is `{% if is_incremental() %}` is
/// built in the variant named by `origin`; a `Plain` request on such a
/// template is treated as `Incremental`, the variant rules run on.
pub fn build_hir(input: &HirInput<'_>, env: &SchemaEnv) -> Lowered<Hir> {
    if input.text.len() > caps::MAX_TEXT_BYTES {
        let detail = format!(
            "the model is {} bytes; the limit is {}",
            input.text.len(),
            caps::MAX_TEXT_BYTES
        );
        return hole_result(
            input,
            HoleKind::Truncated,
            vec![Degradation::new(DegradationKind::CapExceeded, detail)],
        );
    }
    match scan_input(input) {
        Ok(scanned) => build_scanned(input, env, scanned),
        Err(Stop::ControlFlow { detail, span }) => Lowered::declined(vec![Degradation::new(
            DegradationKind::JinjaControlFlow,
            detail,
        )
        .at(span)]),
        Err(Stop::Malformed { detail }) => hole_result(
            input,
            HoleKind::Jinja,
            vec![Degradation::new(DegradationKind::ParseHole, detail)],
        ),
    }
}

fn scan_input(input: &HirInput<'_>) -> Result<Scanned, Stop> {
    match &input.origin {
        SourceOrigin::Template { variant } => scan(input.text, *variant),
        // Compiled SQL has no Jinja; a stray `{{` in a string must not stop it.
        SourceOrigin::Compiled { .. } => Ok(Scanned {
            mask: input.text.to_string(), // boundary: the mask owns its bytes
            regions: Vec::new(),
            variant: TemplateVariant::Plain,
            configs: Vec::new(),
        }),
    }
}

fn build_scanned(input: &HirInput<'_>, env: &SchemaEnv, mut scanned: Scanned) -> Lowered<Hir> {
    let mut lexed = match segment::lex(&scanned.mask, input.dialect) {
        Ok(lexed) => lexed,
        Err(detail) => {
            return hole_result(
                input,
                HoleKind::ParseError,
                vec![Degradation::new(DegradationKind::ParseHole, detail)],
            );
        }
    };
    if lexed.significant > caps::MAX_TOKENS {
        let detail = format!(
            "{} tokens; the limit is {}",
            lexed.significant,
            caps::MAX_TOKENS
        );
        return hole_result(
            input,
            HoleKind::Truncated,
            vec![Degradation::new(DegradationKind::CapExceeded, detail)],
        );
    }
    segment::resolve_positions(&mut lexed, &mut scanned.regions);
    let has_jinja = !scanned.regions.is_empty();
    let Lexed { tokens, pos, .. } = lexed;
    let parsed = parse::parse(tokens, &pos, input.dialect, has_jinja);
    let (hir, degraded) = lower::lower(parsed, &scanned, &pos, input, env);
    finish(input, hir, degraded)
}

fn finish(input: &HirInput<'_>, hir: Hir, degraded: Vec<Degradation>) -> Lowered<Hir> {
    let violations = verify_hir(&hir, input.text);
    let lowered = if degraded.is_empty() {
        Lowered::complete(hir)
    } else {
        Lowered::partial(hir, degraded)
    };
    lowered.with_violations(violations)
}

/// A one-node HIR for input that never reached the parser.
fn hole_result(input: &HirInput<'_>, kind: HoleKind, degraded: Vec<Degradation>) -> Lowered<Hir> {
    let origin = match &input.origin {
        SourceOrigin::Template { variant } => SourceOrigin::Template { variant: *variant },
        // boundary: the origin is owned by the plan (08 §1.5)
        other => other.clone(),
    };
    let mut hir = Hir::new(origin);
    // boundary: the plan owns its config
    hir.config = input.config.clone();
    let span = Span::new(0, input.text.len() as u32);
    let root_scope = hir.scopes.alloc(Scope {
        span,
        parent: None,
        kind: ScopeKind::Root,
        from: Vec::new(),
    });
    let scope = hir.scopes.alloc(Scope {
        span,
        parent: Some(root_scope),
        kind: ScopeKind::Query,
        from: Vec::new(),
    });
    let root = hir.queries.alloc(Query {
        span,
        scope,
        ctes: Vec::new(),
        body: Body::Hole(kind),
        order_by: Vec::new(),
        limit: None,
        outputs: Vec::new(),
    });
    hir.root = Some(root);
    Lowered::partial(hir, degraded)
}
