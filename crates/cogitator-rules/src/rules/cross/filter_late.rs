//! `cross.filter_late`: every consumer of a stored model applies the same
//! filter to it, so the rows are written once and discarded repeatedly
//! (DuckDB `filter_pushdown.cpp`; DataFusion `push_down_filter.rs`, applied
//! across a model boundary the engine cannot see).
//!
//! Views are silent: an engine pushes the predicate through them anyway.
//! Facts come from the consumer's canonical plan, and only predicates the
//! schema job can prove reach here: a single output column compared to
//! literals, applied to every row the consumer reads of this model. A
//! filter written above a join, or over a CTE used twice, proves nothing
//! about the rows the consumer reads. BI tools and hand-written SQL are
//! invisible, so the wording stays "in this project".

use cogitator_ir::common::Materialization;
use cogitator_ir::mir::Mir;
use cogitator_ir::{Level, RuleId};

use super::output::output_span;
use super::select_star_chain::evidence;
use crate::cx::{ColumnFilter, Cx, DagFacts};
use crate::findings::{Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("cross.filter_late"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Dag).with(Need::Mat),
    severity: Severity::Info,
    fix: FixSafety::None,
};

/// Agreement among fewer than two readers is just one model's filter.
const MIN_CONSUMERS: usize = 2;

pub struct FilterLate;

impl Rule for FilterLate {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for FilterLate {
    fn check(&self, _ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        if !matches!(
            cx.mat,
            Some(Materialization::Table | Materialization::Incremental)
        ) {
            return;
        }
        let (Some(hir), Some(dag)) = (cx.hir, cx.dag) else {
            return;
        };
        if dag.consumers.len() < MIN_CONSUMERS {
            return;
        }
        let total = dag.consumers.len();
        for filter in common_filters(dag) {
            out.push(Finding {
                id: META.id,
                span: output_span(hir, &filter.column),
                level: Level::Mir,
                severity: META.severity,
                message: format!(
                    "All {total} models that read this one in this project apply the same \
                     filter on `{}`; filtering here would store fewer rows",
                    filter.column
                ),
                evidence: evidence(dag),
                fixes: Vec::new(),
            });
        }
    }
}

/// The filters every consumer applies, in a fixed order.
fn common_filters(dag: &DagFacts) -> Vec<ColumnFilter> {
    let mut consumers = dag.consumers.values();
    let Some(first) = consumers.next() else {
        return Vec::new();
    };
    let mut common: Vec<ColumnFilter> = first.filters.to_vec();
    for consumer in consumers {
        common.retain(|f| consumer.filters.iter().any(|g| g == f));
    }
    common.sort();
    common.dedup();
    common
}
