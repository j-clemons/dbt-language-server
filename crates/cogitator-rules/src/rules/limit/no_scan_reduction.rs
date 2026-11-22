//! `limit.no_scan_reduction`: `LIMIT` does not reduce the bytes read on
//! a non-clustered BigQuery table.
//!
//! BigQuery's cost best practices: "For non-clustered tables, applying a
//! `LIMIT` clause to a query doesn't affect the amount of data that is
//! read", while on a clustered table "a `LIMIT` clause can reduce the
//! number of bytes scanned". So the rule needs facts for every table the
//! limit reads and none of them clustered; no facts, an open schema or
//! an `Opaque` input makes no claim, nor does `LIMIT 0`, which that page
//! does not cover. Other dialects stay silent.

use cogitator_ir::common::Lit;
use cogitator_ir::lir::{Backend, KeyKind, Lir, PhysId, PhysOp};
use cogitator_ir::mir::{Mir, ScalarId, ScalarKind, TableId};
use cogitator_ir::{Level, RuleId};

use crate::cx::Cx;
use crate::findings::{Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};
use crate::rules::filter::support::{
    plan_evidence, scalar_span, scan_is_closed, touches_opaque, BIGQUERY_COSTS_DOC,
};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("limit.no_scan_reduction"),
    level: RuleLevel::Lir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Dialect).with(Need::Phys),
    severity: Severity::Info,
    fix: FixSafety::None,
};

pub struct NoScanReduction;

impl Rule for NoScanReduction {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Lir> for NoScanReduction {
    fn check(&self, ir: &Lir, cx: &Cx<'_>, out: &mut Findings) {
        let Some(mir) = cx.mir else {
            return;
        };
        if ir.backend != Backend::BigQuery {
            return;
        }
        for node in ir.nodes.values() {
            let PhysOp::Limit {
                input,
                limit: Some(limit),
                ..
            } = &node.op
            else {
                continue;
            };
            if touches_opaque(mir, *limit)
                || is_zero(mir, *limit)
                || !reads_only_non_clustered(ir, mir, cx, *input)
            {
                continue;
            }
            out.push(Finding {
                id: META.id,
                span: scalar_span(mir, *limit),
                level: Level::Lir,
                severity: Severity::Info,
                message: format!(
                    "`LIMIT` does not reduce the bytes read here: BigQuery documents that \
                     for non-clustered tables a `LIMIT` clause doesn't affect the amount \
                     of data that is read, and that you are billed for all bytes the \
                     query reads. See {BIGQUERY_COSTS_DOC}"
                ),
                evidence: plan_evidence(cx.hir),
                fixes: Vec::new(),
            });
        }
    }
}

fn is_zero(mir: &Mir, id: ScalarId) -> bool {
    let Some(ScalarKind::Lit(Lit::Number(text))) = mir.scalars.get(id).map(|s| &s.kind) else {
        return false;
    };
    mir.syms.resolve(*text) == Some("0")
}

/// True when the subtree scans at least one table and every scan is of a
/// closed table known not to be clustered.
fn reads_only_non_clustered(ir: &Lir, mir: &Mir, cx: &Cx<'_>, root: PhysId) -> bool {
    let mut stack = vec![root];
    let mut budget = ir.nodes.len() + 1;
    let mut scanned = false;
    while let Some(id) = stack.pop() {
        if budget == 0 {
            return false;
        }
        budget -= 1;
        let Some(node) = ir.nodes.get(id) else {
            return false;
        };
        match &node.op {
            PhysOp::Opaque { .. } => return false,
            PhysOp::TableScan { table, .. } => {
                scanned = true;
                if !scan_is_closed(mir, node.mir, *table) || !known_non_clustered(cx, *table) {
                    return false;
                }
            }
            op => stack.extend(op.inputs()),
        }
    }
    scanned
}

/// An entry in the facts with no clustering column. No entry says
/// nothing: facts exist only for tables with something known.
fn known_non_clustered(cx: &Cx<'_>, table: TableId) -> bool {
    cx.phys
        .and_then(|p| p.table(table))
        .is_some_and(|t| !t.columns.iter().any(|c| c.key == Some(KeyKind::Cluster)))
}
