//! `filter.partition_or`: an `OR` across columns defeats partition
//! pruning.
//!
//! BigQuery states this on the partitioned-table page: "If `AND` is
//! changed to `OR`, partition pruning won't work". The backend marks
//! exactly that case `Blocked { OrAcrossColumns }` on partition keys.
//! The clustering page does not mention `OR`, so cluster keys make no
//! claim, and neither does any other dialect, a scan with an open schema,
//! or a conjunct holding a Jinja hole.

use cogitator_ir::lir::{Backend, BlockReason, KeyKind, Lir, PruneUse};
use cogitator_ir::{Level, RuleId};

use super::support::{
    col_name, plan_evidence, scalar_span, scan_is_closed, scans, touches_opaque,
    BIGQUERY_PARTITION_DOC,
};
use crate::cx::Cx;
use crate::findings::{Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("filter.partition_or"),
    level: RuleLevel::Lir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Dialect).with(Need::Phys),
    severity: Severity::Warning,
    fix: FixSafety::None,
};

pub struct PartitionOr;

impl Rule for PartitionOr {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Lir> for PartitionOr {
    fn check(&self, ir: &Lir, cx: &Cx<'_>, out: &mut Findings) {
        let Some(mir) = cx.mir else {
            return;
        };
        if ir.backend != Backend::BigQuery {
            return;
        }
        for scan in scans(ir) {
            if !scan_is_closed(mir, scan.mir, scan.table) {
                continue;
            }
            for key in scan.keys {
                let PruneUse::Blocked {
                    by,
                    reason: BlockReason::OrAcrossColumns,
                } = key.usage
                else {
                    continue;
                };
                if key.kind != KeyKind::Partition || touches_opaque(mir, by) {
                    continue;
                }
                let name = col_name(mir, key.col);
                out.push(Finding {
                    id: META.id,
                    span: scalar_span(mir, by),
                    level: Level::Lir,
                    severity: Severity::Warning,
                    message: format!(
                        "This `OR` combines partitioning column `{name}` with another \
                         column. BigQuery documents that partition pruning won't work: a \
                         partition that fails the `{name}` test can still hold matching \
                         rows. Add a condition on `{name}` alone, joined with `AND`. See \
                         {BIGQUERY_PARTITION_DOC}"
                    ),
                    evidence: plan_evidence(cx.hir),
                    fixes: Vec::new(),
                });
            }
        }
    }
}
