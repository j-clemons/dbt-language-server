//! `subq.correlated`: a correlated subquery (DuckDB
//! `flatten_dependent_join.cpp`, `deliminator.cpp`; DataFusion
//! `decorrelate_predicate_subquery.rs`, `scalar_subquery_to_join.rs`).
//!
//! Engines decorrelate most of these, so this is a modest Hint. A plain
//! `[NOT] EXISTS` whose correlation is equalities only is exactly the
//! semi-join they decorrelate reliably, and stays silent.

use std::collections::BTreeSet;

use cogitator_ir::common::BinaryOp;
use cogitator_ir::mir::{Mir, RelId, RelOp, ScalarId, ScalarKind, SubqueryKind};
use cogitator_ir::{Level, RuleId, Span};

use super::scan::{plan_scalars, touches_opaque};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};
use crate::rules::join::support::no_opaque;

pub static META: RuleMeta = RuleMeta {
    id: RuleId("subq.correlated"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE,
    severity: Severity::Hint,
    fix: FixSafety::None,
};

pub struct Correlated;

impl Rule for Correlated {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for Correlated {
    fn check(&self, ir: &Mir, _cx: &Cx<'_>, out: &mut Findings) {
        let mut spans: BTreeSet<Span> = BTreeSet::new();
        for (_, scalar) in plan_scalars(ir) {
            if let Some(finding) = examine(ir, scalar) {
                if spans.insert(finding.span) {
                    out.push(finding);
                }
            }
        }
    }
}

fn examine(mir: &Mir, id: ScalarId) -> Option<Finding> {
    let scalar = mir.scalars.get(id)?;
    let ScalarKind::Subquery {
        kind,
        rel,
        correlated,
    } = &scalar.kind
    else {
        return None;
    };
    if correlated.is_empty() || !no_opaque(mir, *rel) {
        return None;
    }
    if matches!(kind, SubqueryKind::Exists) && plain_equality_correlation(mir, *rel) {
        return None;
    }
    Some(Finding {
        id: META.id,
        span: mir.prov.span(scalar.prov).unwrap_or_default(),
        level: Level::Mir,
        severity: META.severity,
        message: "Correlated subquery: most engines decorrelate this, but one that runs per \
                  outer row can be slow; a join may read more plainly"
            .to_owned(),
        evidence: Evidence::Exact,
        fixes: Vec::new(),
    })
}

/// True when every read of an enclosing column under `rel` is one side of
/// a `column = outer` conjunct, and there is at least one.
fn plain_equality_correlation(mir: &Mir, rel: RelId) -> bool {
    let mut equality = false;
    for id in mir.reachable_from(&[rel]) {
        let Some(node) = mir.rels.get(id) else {
            return false;
        };
        let conjuncts = match &node.op {
            RelOp::Filter { conjuncts: c, .. } | RelOp::Join { on: c, .. } => c.as_slice(),
            _ => &[],
        };
        for scalar in node.op.scalars() {
            if conjuncts.contains(&scalar) && is_equi_outer(mir, scalar) {
                equality = true;
            } else if reads_outer(mir, scalar) {
                return false;
            }
        }
    }
    equality
}

/// `column = outer`, in either order.
fn is_equi_outer(mir: &Mir, id: ScalarId) -> bool {
    let kind = |s: ScalarId| mir.scalars.get(s).map(|n| &n.kind);
    let Some(ScalarKind::Binary {
        op: BinaryOp::Eq,
        l,
        r,
    }) = kind(id)
    else {
        return false;
    };
    matches!(
        (kind(*l), kind(*r)),
        (Some(ScalarKind::Col(_)), Some(ScalarKind::Outer(_)))
            | (Some(ScalarKind::Outer(_)), Some(ScalarKind::Col(_)))
    )
}

/// True when the tree holds an `Outer`, or an opaque scalar that might.
fn reads_outer(mir: &Mir, root: ScalarId) -> bool {
    if touches_opaque(mir, root) {
        return true;
    }
    let mut stack = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match mir.scalars.get(id).map(|s| &s.kind) {
            Some(ScalarKind::Outer(_)) => return true,
            Some(kind) => stack.extend(kind.children()),
            None => return true,
        }
    }
    false
}
