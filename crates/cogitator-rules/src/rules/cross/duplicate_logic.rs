//! `cross.duplicate_logic`: two models contain the same subplan (DuckDB
//! `common_subplan_optimizer.cpp`, DataFusion `common_subexpr_eliminate.rs`,
//! but between models, where the engine cannot share the work).
//!
//! Comparison is on canonical text ([`super::subplan`]) of the canonical
//! plan, never a bare hash: the hash finds candidates and the text confirms
//! them, here and in the schema job that groups them. Only subplans of at
//! least [`super::subplan::MIN_RELS`] operators that do real work compare
//! at all, so `select * from x` cannot match. The finding names the single
//! largest shared subplan; what sits inside it adds nothing.

use cogitator_ir::mir::Mir;
use cogitator_ir::{Level, RuleId};

use super::subplan::subplans_of;
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("cross.duplicate_logic"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Dag),
    severity: Severity::Hint,
    fix: FixSafety::None,
};

pub struct DuplicateLogic;

impl Rule for DuplicateLogic {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for DuplicateLogic {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        let Some(dag) = cx.dag else {
            return;
        };
        if dag.shared_subplans.is_empty() {
            return;
        }
        // largest first, so the first confirmed match is the one to report
        for subplan in subplans_of(ir) {
            let Some(shared) = dag.shared_subplans.get(&subplan.hash) else {
                continue;
            };
            if shared.text != subplan.text {
                continue; // a hash collision is not a finding
            }
            let span = ir.rels.get(subplan.at).and_then(|r| ir.prov.span(r.prov));
            let mut names: Vec<&str> = shared.models.iter().map(|n| &**n).collect();
            if shared.total > names.len() {
                names.push("…");
            }
            out.push(Finding {
                id: META.id,
                span: span.unwrap_or_default(),
                level: Level::Mir,
                severity: META.severity,
                message: format!(
                    "The same {}-operator query logic also appears in {} in this project; \
                     consider sharing one model",
                    subplan.size,
                    names.join(", ")
                ),
                evidence: Evidence::Exact,
                fixes: Vec::new(),
            });
            return;
        }
    }
}
