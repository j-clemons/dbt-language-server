//! `join.cartesian`: a join with no key tying its inputs together
//! (DuckDB `pushdown/pushdown_cross_product.cpp`, DataFusion
//! `eliminate_cross_join.rs`).
//!
//! Canonicalisation moves one-sided ON conditions into an input, so an
//! empty `on` on the canonical plan says nothing about what was written.
//! An empty `on` is therefore only reported once the HIR shows the
//! author wrote no condition tying the inputs together.

use std::collections::{BTreeMap, BTreeSet};

use cogitator_ir::common::JoinKind;
use cogitator_ir::hir::{ExprId, ExprKind, FromId, FromKind, Hir, JoinCond, Resolution};
use cogitator_ir::mir::{ColId, HirNode, Mir, Rel, RelId, RelOp, ScalarId};
use cogitator_ir::{Level, RuleId};

use super::support::{expr, from_leaves, hir_ref, is_one_row, no_opaque, reads};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("join.cartesian"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE,
    severity: Severity::Warning,
    fix: FixSafety::None,
};

pub struct Cartesian;

impl Rule for Cartesian {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for Cartesian {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        let reachable = ir.reachable();
        let parents = parents_of(ir, &reachable);
        for id in reachable {
            if let Some(finding) = examine(ir, id, &parents, cx) {
                out.push(finding);
            }
        }
    }
}

fn parents_of(mir: &Mir, ids: &[RelId]) -> BTreeMap<RelId, Vec<RelId>> {
    let mut parents: BTreeMap<RelId, Vec<RelId>> = BTreeMap::new();
    for id in ids {
        if let Some(rel) = mir.rels.get(*id) {
            for input in rel.op.inputs() {
                parents.entry(input).or_default().push(*id);
            }
        }
    }
    parents
}

fn examine(
    mir: &Mir,
    id: RelId,
    parents: &BTreeMap<RelId, Vec<RelId>>,
    cx: &Cx<'_>,
) -> Option<Finding> {
    let rel = mir.rels.get(id)?;
    let RelOp::Join { kind, l, r, on } = &rel.op else {
        return None;
    };
    if matches!(kind, JoinKind::Semi | JoinKind::Anti) {
        return None;
    }
    let hir = cx.hir?;
    // "No condition" is about the written ON and WHERE, not about every
    // column: open scans are fine, an `Opaque` input is not.
    if !no_opaque(mir, *l) || !no_opaque(mir, *r) || is_one_row(mir, *l) || is_one_row(mir, *r) {
        return None;
    }
    let (left, right) = (columns(mir, *l)?, columns(mir, *r)?);
    if on.iter().any(|s| {
        let facts = reads(mir, *s);
        facts.opaque || relates(&facts.cols, &left, &right)
    }) {
        return None;
    }
    if !keyless_as_written(mir, hir, rel, *kind, on) {
        return None;
    }
    if related_above(mir, id, &left, &right, parents) {
        return None;
    }
    Some(Finding {
        id: META.id,
        span: mir.prov.span(rel.prov).unwrap_or_default(),
        level: Level::Mir,
        severity: META.severity,
        message: "This join has no condition relating its two inputs, so every row of one pairs \
                  with every row of the other"
            .to_owned(),
        evidence: Evidence::Exact,
        fixes: Vec::new(),
    })
}

fn columns(mir: &Mir, rel: RelId) -> Option<BTreeSet<ColId>> {
    Some(mir.rels.get(rel)?.out.iter().copied().collect())
}

fn relates(cols: &BTreeSet<ColId>, left: &BTreeSet<ColId>, right: &BTreeSet<ColId>) -> bool {
    cols.iter().any(|c| left.contains(c)) && cols.iter().any(|c| right.contains(c))
}

/// What the author wrote, for a join whose canonical `on` relates
/// nothing. `CROSS JOIN` and `ON TRUE` say the product is intended; an
/// `ON` that reads columns is the claim only if it reads one side.
fn keyless_as_written(mir: &Mir, hir: &Hir, rel: &Rel, kind: JoinKind, on: &[ScalarId]) -> bool {
    let node = hir_ref(mir, rel.prov).map(|r| r.node);
    let Some(HirNode::From(from)) = node else {
        // A comma join: its provenance is the SELECT. Keyless by syntax.
        return kind == JoinKind::Cross && on.is_empty() && matches!(node, Some(HirNode::Query(_)));
    };
    let Some(FromKind::Join {
        kind: written,
        l,
        r,
        on: cond,
    }) = hir.from.get(from).map(|f| &f.kind)
    else {
        return false;
    };
    if *written == JoinKind::Cross {
        return false;
    }
    match cond {
        JoinCond::None => true,
        JoinCond::Using(_) | JoinCond::Natural => false,
        JoinCond::On(e) => written_on_is_one_sided(hir, *e, *l, *r),
    }
}

/// True when the written ON reads columns from exactly one side and
/// every reference is bound. `OpenSchema` names its FROM item, so its side
/// is known; any other unresolved reference blocks the claim.
fn written_on_is_one_sided(hir: &Hir, on: ExprId, l: FromId, r: FromId) -> bool {
    let (left, right) = (from_leaves(hir, l), from_leaves(hir, r));
    let (mut in_left, mut in_right) = (false, false);
    let mut stack = vec![on];
    while let Some(id) = stack.pop() {
        let Some(node) = expr(hir, id) else {
            return false;
        };
        match &node.kind {
            ExprKind::Col(c) => match hir.cols.get(*c).and_then(|c| c.res.as_ref()) {
                Some(Resolution::Bound { from, .. } | Resolution::OpenSchema(from)) => {
                    in_left |= left.contains(from);
                    in_right |= right.contains(from);
                }
                _ => return false,
            },
            ExprKind::Jinja(_) | ExprKind::Opaque { .. } | ExprKind::Hole => return false,
            kind if kind.subquery().is_some() => return false,
            _ => {}
        }
        stack.extend(node.kind.children());
    }
    in_left != in_right
}

/// A condition above the join that reads both inputs: `FROM a, b WHERE
/// a.id = b.id`, or one canonicalisation could not push down. Columns are
/// followed through Projects, whose outputs may rename them.
fn related_above(
    mir: &Mir,
    join: RelId,
    left: &BTreeSet<ColId>,
    right: &BTreeSet<ColId>,
    parents: &BTreeMap<RelId, Vec<RelId>>,
) -> bool {
    let (mut left, mut right) = (left.clone(), right.clone());
    let mut seen = BTreeSet::from([join]);
    let mut stack: Vec<RelId> = parents.get(&join).cloned().unwrap_or_default();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(rel) = mir.rels.get(id) else {
            continue;
        };
        match &rel.op {
            RelOp::Filter { conjuncts, .. } | RelOp::Join { on: conjuncts, .. } => {
                let found = conjuncts.iter().any(|s| {
                    let facts = reads(mir, *s);
                    facts.opaque || relates(&facts.cols, &left, &right)
                });
                if found {
                    return true;
                }
            }
            RelOp::Project { items, .. } => {
                for (col, scalar) in items {
                    let facts = reads(mir, *scalar);
                    if facts.cols.iter().any(|c| left.contains(c)) {
                        left.insert(*col);
                    }
                    if facts.cols.iter().any(|c| right.contains(c)) {
                        right.insert(*col);
                    }
                }
            }
            RelOp::Sort { .. } | RelOp::Limit { .. } | RelOp::Distinct(_) => {}
            _ => continue,
        }
        stack.extend(parents.get(&id).cloned().unwrap_or_default());
    }
    false
}
