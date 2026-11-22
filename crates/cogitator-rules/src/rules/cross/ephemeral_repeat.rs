//! `cross.ephemeral_repeat`: an ephemeral model compiled into several
//! models repeats its work in each of them (DuckDB `cte_inlining.cpp`,
//! whose concern is the same fan-out inside one query).
//!
//! The count is of the non-ephemeral models it is compiled into, ephemeral
//! chains looked through. Fires from [`THRESHOLD`] of them and only when
//! the body does work worth repeating: a bare projection or filter costs
//! the engine nothing to inline. `Need::Dag` is withheld when a consumer
//! could not be read, and the count is only of this project's models.

use cogitator_ir::common::Materialization;
use cogitator_ir::mir::Mir;
use cogitator_ir::{Level, RuleId, Span};

use super::subplan::is_work;
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("cross.ephemeral_repeat"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Dag).with(Need::Mat),
    severity: Severity::Hint,
    fix: FixSafety::None,
};

/// Three models: at two, keeping the logic in one ephemeral model is the
/// ordinary reason to have it, and a table would add a build step and
/// storage to save one recomputation. From three the recomputation is
/// the repeated cost worth naming, and the rule is only a hint.
pub const THRESHOLD: usize = 3;

/// How many of the compiling models the message names.
const NAMED: usize = 5;

pub struct EphemeralRepeat;

impl Rule for EphemeralRepeat {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for EphemeralRepeat {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        if cx.mat != Some(Materialization::Ephemeral) {
            return;
        }
        let Some(dag) = cx.dag else {
            return;
        };
        let count = dag.inlined_into.len();
        if count < THRESHOLD || !does_work(ir) {
            return;
        }
        let mut names: Vec<&str> = dag.inlined_into.iter().take(NAMED).map(|n| &**n).collect();
        let more = count.saturating_sub(names.len());
        if more > 0 {
            names.push("…");
        }
        out.push(Finding {
            id: META.id,
            span: root_span(ir, cx),
            level: Level::Mir,
            severity: META.severity,
            message: format!(
                "This ephemeral model is compiled into {count} models in this project ({}); \
                 each one repeats its work, so a view or table may be cheaper",
                names.join(", ")
            ),
            evidence: Evidence::Exact,
            fixes: Vec::new(),
        });
    }
}

fn does_work(ir: &Mir) -> bool {
    ir.reachable()
        .iter()
        .any(|id| ir.rels.get(*id).is_some_and(|rel| is_work(&rel.op)))
}

fn root_span(ir: &Mir, cx: &Cx<'_>) -> Span {
    let hir_root = cx
        .hir
        .and_then(|h| h.root.and_then(|r| h.queries.get(r)))
        .map(|q| q.span);
    hir_root
        .or_else(|| ir.rels.get(ir.root).and_then(|r| ir.prov.span(r.prov)))
        .unwrap_or_default()
}
