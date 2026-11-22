//! `cross.unused_column`: a stored output column nothing downstream in
//! this project reads (DuckDB `remove_unused_columns.cpp`), so its bytes
//! are written for nothing.
//!
//! Views and ephemerals are silent: engines prune them, so the column
//! costs nothing. Star expansions are `cross.select_star_chain`'s, because
//! there is no individual column to point at; when that rule reports the
//! model the whole model is covered. Columns the model's own config uses
//! (unique, partition, cluster and index keys) are read by the build.
//! BI tools and hand-written SQL are invisible, so the wording stays
//! "in this project".

use std::collections::BTreeSet;

use cogitator_ir::common::Materialization;
use cogitator_ir::hir::{Hir, Query, SelectItem};
use cogitator_ir::mir::Mir;
use cogitator_ir::{Level, RuleId, Span, Symbol};

use super::output::output_open;
use super::select_star_chain::{covers, evidence, lower, root_select, used_columns};
use crate::cx::{Cx, DagFacts};
use crate::findings::{Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("cross.unused_column"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Dag).with(Need::Mat),
    severity: Severity::Hint,
    fix: FixSafety::None,
};

/// Past this many columns on one model the last finding says how many
/// more there are: a wide model should not flood the editor.
const MAX_FINDINGS: usize = 64;

pub struct UnusedColumn;

impl Rule for UnusedColumn {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for UnusedColumn {
    fn check(&self, _ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        let (Some(hir), Some(dag)) = (cx.hir, cx.dag) else {
            return;
        };
        for finding in findings(cx, hir, dag).unwrap_or_default() {
            out.push(finding);
        }
    }
}

fn findings(cx: &Cx<'_>, hir: &Hir, dag: &DagFacts) -> Option<Vec<Finding>> {
    if !matches!(
        cx.mat,
        Some(Materialization::Table | Materialization::Incremental)
    ) || dag.downstream_columns.is_empty()
        || hir.config.contract_enforced
    {
        return None; // no reader, or the column list is a published contract
    }
    let root = hir.root?;
    if output_open(hir, root) {
        return None;
    }
    let select = root_select(hir, root)?;
    let query = hir.queries.get(root)?;
    let stars: BTreeSet<u32> = select
        .items
        .iter()
        .filter_map(|item| match item {
            SelectItem::Star { span, .. } => Some(span.start),
            _ => None,
        })
        .collect();
    let used = used_columns(dag);
    let keys = config_columns(hir);
    let unused: Vec<(&str, Span)> = named_outputs(hir, query, &stars)
        .into_iter()
        .filter(|(name, _)| !used.contains(&lower(name)) && !keys.contains(&lower(name)))
        .collect();
    if unused.is_empty() || covers(cx) {
        return None;
    }
    let total = unused.len();
    let extra = total.saturating_sub(MAX_FINDINGS);
    let reported = unused.into_iter().take(MAX_FINDINGS);
    Some(
        reported
            .enumerate()
            .map(|(i, (name, span))| {
                let suffix = if extra > 0 && i + 1 == MAX_FINDINGS {
                    format!(" ({extra} more columns like this are not shown)")
                } else {
                    String::new()
                };
                Finding {
                    id: META.id,
                    span,
                    level: Level::Mir,
                    severity: META.severity,
                    message: format!(
                        "No downstream model or test in this project reads `{name}`; it is \
                         stored for nothing unless something outside the project uses it{suffix}"
                    ),
                    evidence: evidence(dag),
                    fixes: Vec::new(),
                }
            })
            .collect(),
    )
}

/// `(name, span)` of each explicitly named output, skipping star
/// expansions, whose outputs carry the star's span.
fn named_outputs<'h>(hir: &'h Hir, query: &Query, stars: &BTreeSet<u32>) -> Vec<(&'h str, Span)> {
    query
        .outputs
        .iter()
        .filter(|o| !stars.contains(&o.span.start))
        .filter_map(|o| o.name.map(|n| (hir.syms.text(n), o.span)))
        .collect()
}

/// Columns the model's own config reads: writing them is not waste.
fn config_columns(hir: &Hir) -> BTreeSet<String> {
    let name = |s: &Symbol| lower(hir.syms.text(*s));
    let config = &hir.config;
    config
        .unique_key
        .iter()
        .chain(&config.partition_by)
        .chain(&config.cluster_by)
        .chain(config.indexes.iter().flatten())
        .map(name)
        .collect()
}
