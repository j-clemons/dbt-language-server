//! `join.left_nullified`: a filter that rejects the NULLs a LEFT JOIN
//! adds makes the join inner (DuckDB `outer_join_simplification.cpp`,
//! DataFusion `eliminate_outer_join.rs`).

use std::collections::BTreeSet;

use cogitator_ir::common::JoinKind;
use cogitator_ir::mir::{ColId, Mir, RelId, RelOp, ScalarId};
use cogitator_ir::{Level, RuleId, Span};

use super::left_fix;
use super::null_reject::rejects;
use super::support::{join_from, no_opaque, reads};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings, Fix};
use crate::meta::{FixSafety, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("join.left_nullified"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE,
    severity: Severity::Warning,
    fix: FixSafety::Mixed,
};

pub struct LeftNullified;

impl Rule for LeftNullified {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for LeftNullified {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        for id in ir.reachable() {
            if let Some(finding) = examine(ir, id, cx) {
                out.push(finding);
            }
        }
    }
}

/// A Filter directly above an outer join, which is where canonicalisation
/// leaves any predicate that may not move below that join.
fn examine(mir: &Mir, id: RelId, cx: &Cx<'_>) -> Option<Finding> {
    let RelOp::Filter { input, conjuncts } = &mir.rels.get(id)?.op else {
        return None;
    };
    let join = mir.rels.get(*input)?;
    let RelOp::Join { kind, l, r, .. } = &join.op else {
        return None;
    };
    let (word, null_rel) = match kind {
        JoinKind::Left => ("LEFT", *r),
        JoinKind::Right => ("RIGHT", *l),
        _ => return None,
    };
    // Open scans are fine: a conjunct can only reach `first_rejecting`'s
    // null side through a column the binder pinned to that FROM item
    // (qualified, or the only item that can supply it); an unqualified
    // name that could belong to either side is `Candidates`, so opaque.
    if !no_opaque(mir, *l) || !no_opaque(mir, *r) {
        return None;
    }
    let null_side: BTreeSet<ColId> = mir.rels.get(null_rel)?.out.iter().copied().collect();
    let conjunct = first_rejecting(mir, conjuncts, &null_side)?;
    let from = join_from(mir, join.prov);
    if written_without_condition(cx, from) {
        return None;
    }
    let scalar = mir.scalars.get(conjunct)?;
    let span = mir
        .prov
        .span(scalar.prov)
        .unwrap_or_else(|| fallback_span(mir, id));
    Some(Finding {
        id: META.id,
        span,
        level: Level::Mir,
        severity: META.severity,
        message: format!(
            "This filter rejects the NULLs this {word} JOIN adds, so the join behaves as an INNER JOIN"
        ),
        evidence: Evidence::Exact,
        fixes: fixes(mir, cx, from, *kind, conjunct),
    })
}

/// The first conjunct that cannot pass with `null_side` all NULL and
/// that reads the side, so `where false` does not count.
fn first_rejecting(
    mir: &Mir,
    conjuncts: &[ScalarId],
    null_side: &BTreeSet<ColId>,
) -> Option<ScalarId> {
    conjuncts.iter().copied().find(|c| {
        let facts = reads(mir, *c);
        !facts.opaque
            && facts.cols.iter().any(|col| null_side.contains(col))
            && rejects(mir, *c, null_side)
    })
}

/// `OUTER APPLY` lowers to a keyless Left join; it is a different
/// operator and has nothing to rewrite to INNER.
fn written_without_condition(cx: &Cx<'_>, from: Option<cogitator_ir::hir::FromId>) -> bool {
    let (Some(hir), Some(from)) = (cx.hir, from) else {
        return false;
    };
    matches!(
        hir.from.get(from).map(|f| &f.kind),
        Some(cogitator_ir::hir::FromKind::Join {
            on: cogitator_ir::hir::JoinCond::None,
            ..
        })
    )
}

fn fallback_span(mir: &Mir, id: RelId) -> Span {
    mir.rels
        .get(id)
        .and_then(|r| mir.prov.span(r.prov))
        .unwrap_or_default()
}

fn fixes(
    mir: &Mir,
    cx: &Cx<'_>,
    from: Option<cogitator_ir::hir::FromId>,
    kind: JoinKind,
    conjunct: ScalarId,
) -> Vec<Fix> {
    let (Some(hir), Some(text), Some(from)) = (cx.hir, cx.text, from) else {
        return Vec::new();
    };
    // P first: only a P fix may be preferred or join `source.fixAll`.
    [
        left_fix::inner_join(hir, text, from, kind),
        left_fix::move_to_on(hir, mir, text, from, conjunct),
    ]
    .into_iter()
    .flatten()
    .collect()
}
