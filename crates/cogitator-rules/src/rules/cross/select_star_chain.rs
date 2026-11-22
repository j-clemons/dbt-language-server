//! `cross.select_star_chain`: a stored model selects `*` but only some of
//! those columns are read downstream (DuckDB `remove_unused_columns.cpp`,
//! DataFusion `optimize_projections/`), so the rest is written for nothing.
//!
//! Only table and incremental boundaries store the columns. Engines see
//! through views and inline ephemerals, so those are silent. Demand is
//! what the rest of this project reads: `Need::Dag` is withheld when a
//! downstream could not be read, and exposures or BI tools remain
//! invisible, which is why the message says "in this project" and the
//! expand fix is N.

use std::collections::BTreeSet;

use cogitator_ir::common::Materialization;
use cogitator_ir::hir::{Body, Hir, Query, QueryId, Select, SelectItem};
use cogitator_ir::mir::Mir;
use cogitator_ir::{Level, RuleId, Span};

use super::output::output_open;
use crate::cx::{Cx, DagFacts};
use crate::findings::{Evidence, Finding, Findings, Fix, FixEdit};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("cross.select_star_chain"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE
        .with(Need::Schema)
        .with(Need::Dag)
        .with(Need::Mat),
    severity: Severity::Info,
    fix: FixSafety::ChangesIntent,
};

pub struct SelectStarChain;

impl Rule for SelectStarChain {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for SelectStarChain {
    fn check(&self, _ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        if let Some(finding) = examine(cx) {
            out.push(finding);
        }
    }
}

/// True when this rule reports the model's columns, so another rule
/// must not report them one by one.
pub(super) fn covers(cx: &Cx<'_>) -> bool {
    examine(cx).is_some()
}

fn examine(cx: &Cx<'_>) -> Option<Finding> {
    if !matches!(
        cx.mat,
        Some(Materialization::Table | Materialization::Incremental)
    ) {
        return None;
    }
    let (hir, dag) = (cx.hir?, cx.dag?);
    if dag.downstream_columns.is_empty() {
        return None; // nothing in the project reads it: no claim to make
    }
    let root = hir.root?;
    if output_open(hir, root) {
        return None;
    }
    let select = root_select(hir, root)?;
    let star = sole_star(select)?;
    let query = hir.queries.get(root)?;
    let names: Vec<&str> = query
        .outputs
        .iter()
        .map(|o| o.name.map(|n| hir.syms.text(n)))
        .collect::<Option<_>>()?;
    let used = used_columns(dag);
    let total = names.len();
    let used_count = names.iter().filter(|n| used.contains(&lower(n))).count();
    if used_count >= total {
        return None;
    }
    Some(Finding {
        id: META.id,
        span: star.span,
        level: Level::Mir,
        severity: META.severity,
        message: format!(
            "Only {used_count} of {total} columns are used downstream in this project; \
             the rest are stored for nothing"
        ),
        evidence: evidence(dag),
        fixes: expand_fix(cx, hir, query, &star, &used)
            .into_iter()
            .collect(),
    })
}

pub(super) fn evidence(dag: &DagFacts) -> Evidence {
    match &dag.catalog_generated_at {
        Some(generated_at) => Evidence::Catalog {
            generated_at: generated_at.clone(),
        },
        None => Evidence::Exact,
    }
}

pub(super) fn root_select(hir: &Hir, root: QueryId) -> Option<&Select> {
    match &hir.queries.get(root)?.body {
        Body::Select(select) => Some(select),
        _ => None,
    }
}

struct Star {
    span: Span,
    plain: bool,
}

/// The select list's only star. Two stars make "which columns" unclear.
fn sole_star(select: &Select) -> Option<Star> {
    let mut stars = select.items.iter().filter_map(|item| match item {
        SelectItem::Star {
            span,
            qualifier,
            exclude,
            replace,
            ..
        } => Some(Star {
            span: *span,
            plain: qualifier.is_none() && exclude.is_empty() && replace.is_empty(),
        }),
        _ => None,
    });
    let star = stars.next()?;
    stars.next().is_none().then_some(star)
}

pub(super) fn used_columns(dag: &DagFacts) -> BTreeSet<String> {
    dag.downstream_columns
        .values()
        .flatten()
        .chain(&dag.tested_columns)
        .map(|c| lower(c))
        .collect()
}

pub(super) fn lower(name: &str) -> String {
    name.to_lowercase()
}

/// Replaces a bare `*` with the used columns, in star order. Withheld
/// unless the star's span holds exactly `*` and every name is plain.
fn expand_fix(
    cx: &Cx<'_>,
    hir: &Hir,
    query: &Query,
    star: &Star,
    used: &BTreeSet<String>,
) -> Option<Fix> {
    if !star.plain || !star_text_is_exact(cx.text?, star.span) {
        return None;
    }
    let mut kept = Vec::new();
    for output in query.outputs.iter().filter(|o| o.span == star.span) {
        let name = hir.syms.text(output.name?);
        if used.contains(&lower(name)) {
            if !is_plain_identifier(name) {
                return None;
            }
            kept.push(name);
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(Fix {
        title: format!(
            "Expand * to the {} column{} used downstream (freezes the list)",
            kept.len(),
            if kept.len() == 1 { "" } else { "s" }
        ),
        safety: FixSafety::ChangesIntent,
        edits: vec![FixEdit {
            span: star.span,
            new_text: kept.join(", "),
        }],
    })
}

fn star_text_is_exact(text: &str, span: Span) -> bool {
    text.get(span.start as usize..span.end as usize) == Some("*")
}

/// Emittable without quoting in every dialect the backends cover.
fn is_plain_identifier(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "all",
        "and",
        "as",
        "asc",
        "between",
        "by",
        "case",
        "cast",
        "check",
        "column",
        "create",
        "cross",
        "current",
        "default",
        "desc",
        "distinct",
        "else",
        "end",
        "except",
        "exists",
        "false",
        "fetch",
        "for",
        "from",
        "full",
        "group",
        "having",
        "in",
        "inner",
        "insert",
        "intersect",
        "into",
        "is",
        "join",
        "left",
        "like",
        "limit",
        "natural",
        "not",
        "null",
        "offset",
        "on",
        "or",
        "order",
        "outer",
        "right",
        "select",
        "set",
        "table",
        "then",
        "to",
        "true",
        "union",
        "using",
        "when",
        "where",
        "with",
    ];
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !RESERVED.contains(&name.to_ascii_lowercase().as_str())
}
