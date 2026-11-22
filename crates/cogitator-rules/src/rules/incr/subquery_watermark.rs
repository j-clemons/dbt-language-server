//! `incr.subquery_watermark`: `> (select max(..) from {{ this }})` on a
//! pruning key.
//!
//! BigQuery and Snowflake each document that a subquery value does not
//! prune. Saying so takes two facts: the template shows the watermark
//! (the `Incremental` variant, where `{{ this }}` survives) and the
//! backend judged a key `Blocked { SubqueryValue }`. Only LIR holds the
//! key verdicts, so the finding comes from there and reads the variant
//! from `Cx::hir`; the HIR pass adds nothing and stays empty, because
//! reporting from both would duplicate the finding.
//!
//! BigQuery's clustering page does not mention subqueries, so a cluster
//! key there is `Unknown` and makes no claim. No autofix exists: the
//! replacement depends on the model's incremental strategy.

use cogitator_ir::common::BinaryOp;
use cogitator_ir::hir::Hir;
use cogitator_ir::lir::{Backend, BlockReason, Lir, PruneUse};
use cogitator_ir::mir::{ColId, Mir, RelId, RelOp, ScalarId, ScalarKind, SubqueryKind, TableRef};
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::{Level, RuleId};

use crate::cx::Cx;
use crate::findings::{Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};
use crate::rules::filter::support::{
    col_name, plan_evidence, reads_col, scalar_span, scan_is_closed, scans, touches_opaque,
    BIGQUERY_PARTITION_DOC, SNOWFLAKE_CLUSTERING_DOC,
};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("incr.subquery_watermark"),
    level: RuleLevel::HirAndLir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Dialect).with(Need::Phys),
    severity: Severity::Warning,
    fix: FixSafety::None,
};

pub struct SubqueryWatermark;

impl Rule for SubqueryWatermark {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Hir> for SubqueryWatermark {
    fn check(&self, _ir: &Hir, _cx: &Cx<'_>, _out: &mut Findings) {}
}

impl Detect<Lir> for SubqueryWatermark {
    fn check(&self, ir: &Lir, cx: &Cx<'_>, out: &mut Findings) {
        let incremental = matches!(
            cx.hir.map(|h| &h.origin),
            Some(SourceOrigin::Template {
                variant: TemplateVariant::Incremental
            })
        );
        let (Some(mir), true) = (cx.mir, incremental) else {
            return;
        };
        let (severity, advice) = match ir.backend {
            Backend::BigQuery => (Severity::Warning, bigquery_advice()),
            Backend::Snowflake => (Severity::Info, snowflake_advice()),
            Backend::DuckDb | Backend::Postgres | Backend::Generic => return,
        };
        for scan in scans(ir) {
            if !scan_is_closed(mir, scan.mir, scan.table) {
                continue;
            }
            for key in scan.keys {
                let PruneUse::Blocked {
                    by,
                    reason: BlockReason::SubqueryValue,
                } = key.usage
                else {
                    continue;
                };
                if !is_watermark(mir, by, key.col) {
                    continue;
                }
                out.push(Finding {
                    id: META.id,
                    span: scalar_span(mir, by),
                    level: Level::Lir,
                    severity,
                    message: format!(
                        "`{}` is compared with a subquery on `{{{{ this }}}}`: {advice}",
                        col_name(mir, key.col)
                    ),
                    evidence: plan_evidence(cx.hir),
                    fixes: Vec::new(),
                });
            }
        }
    }
}

fn bigquery_advice() -> String {
    format!(
        "BigQuery documents that a value computed by a subquery does not prune \
         partitions. With the `insert_overwrite` strategy, dbt's `_dbt_max_partition` \
         holds the latest partition as a scripting variable. See {BIGQUERY_PARTITION_DOC}"
    )
}

fn snowflake_advice() -> String {
    format!(
        "Snowflake documents that it does not prune micro-partitions from a predicate \
         with a subquery, even a constant one. See {SNOWFLAKE_CLUSTERING_DOC}"
    )
}

/// `col > (… max(..) … {{ this }} …)`, in either operand order. Holes
/// around the comparison leave its shape unknown, so they do not match.
fn is_watermark(mir: &Mir, by: ScalarId, col: ColId) -> bool {
    if touches_opaque(mir, by) {
        return false;
    }
    let Some(ScalarKind::Binary { op, l, r }) = mir.scalars.get(by).map(|s| &s.kind) else {
        return false;
    };
    let value_side = match op {
        BinaryOp::Gt | BinaryOp::GtEq if reads_col(mir, *l, col) => *r,
        BinaryOp::Lt | BinaryOp::LtEq if reads_col(mir, *r, col) => *l,
        _ => return false,
    };
    subquery_rels(mir, value_side)
        .into_iter()
        .any(|rel| reads_max_of_this(mir, rel))
}

fn subquery_rels(mir: &Mir, root: ScalarId) -> Vec<RelId> {
    let mut rels = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(scalar) = mir.scalars.get(id) else {
            continue;
        };
        if let ScalarKind::Subquery {
            kind: SubqueryKind::Scalar,
            rel,
            ..
        } = scalar.kind
        {
            rels.push(rel);
        }
        stack.extend(scalar.kind.children());
    }
    rels
}

/// The subquery reads `{{ this }}` and computes a `max`, through CTEs.
fn reads_max_of_this(mir: &Mir, rel: RelId) -> bool {
    let reachable = mir.reachable_from(&[rel]);
    let op = |id: &RelId| mir.rels.get(*id).map(|r| &r.op);
    let reads_this = reachable.iter().any(|id| {
        let Some(RelOp::Scan { table, .. }) = op(id) else {
            return false;
        };
        matches!(
            mir.tables.get(*table).map(|t| &t.target),
            Some(TableRef::This)
        )
    });
    let computes_max = reachable.iter().any(|id| {
        let Some(RelOp::Aggregate { aggs, .. }) = op(id) else {
            return false;
        };
        aggs.iter().any(|(_, call)| {
            mir.syms
                .resolve(call.func)
                .is_some_and(|f| f.eq_ignore_ascii_case("max"))
        })
    });
    reads_this && computes_max
}
