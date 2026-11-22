//! What the join and set rules share: how far a plan can be trusted,
//! what a predicate reads, and how to get back to the authored HIR.

use std::collections::BTreeSet;

use cogitator_ir::hir::{Expr, ExprId, ExprKind, FromId, FromKind, Hir};
use cogitator_ir::mir::{
    ColId, HirNode, HirRef, Mir, Origin, ProvId, RelId, RelOp, ScalarId, ScalarKind,
};
use cogitator_ir::Span;

/// The `HirRef` behind `id`, following `Derived` chains like
/// `ProvTable::span`, bounded by the table size.
pub(crate) fn hir_ref(mir: &Mir, id: ProvId) -> Option<HirRef> {
    let mut at = id;
    for _ in 0..=mir.prov.entries.len() {
        match mir.prov.entries.get(at)? {
            Origin::Hir(r) => return Some(*r),
            Origin::Derived { from, .. } => at = *from.first()?,
        }
    }
    None
}

/// The FROM item behind a `Join`'s provenance, if lowering had one.
pub(crate) fn join_from(mir: &Mir, prov: ProvId) -> Option<FromId> {
    match hir_ref(mir, prov)?.node {
        HirNode::From(id) => Some(id),
        _ => None,
    }
}

/// The row source of `SELECT` without FROM. An `Opaque` in the arena but
/// not a hole in the input (lowering adds no `Degradation` for it).
pub(crate) fn is_singleton(op: &RelOp) -> bool {
    matches!(
        op,
        RelOp::Opaque { inputs, out, open: false, hole }
            if inputs.is_empty() && out.is_empty() && hole.span == Span::default()
    )
}

/// True when every relation under `rel` (CTE bodies and subqueries
/// included) has a closed schema and none is `Opaque`. For claims that
/// range over every column of the input (`join.distinct_fanout`); claims
/// about what is written use [`no_opaque`].
pub(crate) fn is_closed(mir: &Mir, rel: RelId) -> bool {
    mir.reachable_from(&[rel])
        .iter()
        .all(|id| match mir.rels.get(*id).map(|r| &r.op) {
            Some(RelOp::Scan { open, .. }) => !*open,
            Some(op @ RelOp::Opaque { .. }) => is_singleton(op),
            Some(_) => true,
            None => false,
        })
}

/// True when nothing under `rel` is an `Opaque`. Open scans are allowed:
/// a claim about what is *written* (a keyless join, a filter on a named
/// column of a FROM item, a UNION) survives not knowing every column, but
/// not a node that hides what it reads.
pub(crate) fn no_opaque(mir: &Mir, rel: RelId) -> bool {
    mir.reachable_from(&[rel])
        .iter()
        .all(|id| match mir.rels.get(*id).map(|r| &r.op) {
            Some(op @ RelOp::Opaque { .. }) => is_singleton(op),
            Some(_) => true,
            None => false,
        })
}

/// True when `rel` provably produces at most one row.
pub(crate) fn is_one_row(mir: &Mir, rel: RelId) -> bool {
    let mut at = rel;
    for _ in 0..=mir.rels.len() {
        let Some(node) = mir.rels.get(at) else {
            return false;
        };
        match &node.op {
            op @ RelOp::Opaque { .. } => return is_singleton(op),
            RelOp::Aggregate { keys, sets, .. } => return keys.is_empty() && sets.is_none(),
            RelOp::Limit {
                limit,
                offset,
                input,
            } => {
                let at_most_one = offset.is_none()
                    && limit
                        .and_then(|s| mir.scalars.get(s))
                        .is_some_and(|s| lit_at_most_one(mir, &s.kind));
                if at_most_one {
                    return true;
                }
                at = *input;
            }
            RelOp::Project { input, .. }
            | RelOp::Filter { input, .. }
            | RelOp::Sort { input, .. }
            | RelOp::Window { input, .. }
            | RelOp::Distinct(input) => at = *input,
            RelOp::CteRef { cte, .. } => match mir.ctes.get(*cte) {
                Some(def) => at = def.body,
                None => return false,
            },
            _ => return false,
        }
    }
    false
}

fn lit_at_most_one(mir: &Mir, kind: &ScalarKind) -> bool {
    match kind {
        ScalarKind::Lit(cogitator_ir::common::Lit::Number(sym)) => {
            matches!(mir.syms.text(*sym), "0" | "1")
        }
        _ => false,
    }
}

/// What a scalar tree reads.
#[derive(Debug, Default)]
pub(crate) struct Reads {
    pub cols: BTreeSet<ColId>,
    /// Reaches an `Opaque` scalar or a subquery over something open.
    pub opaque: bool,
}

/// Columns read by `root`, including the outer columns of correlated
/// subqueries, which are the only way those can tie two inputs together.
pub(crate) fn reads(mir: &Mir, root: ScalarId) -> Reads {
    let mut out = Reads::default();
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = mir.scalars.get(id) else {
            out.opaque = true;
            continue;
        };
        match &node.kind {
            ScalarKind::Col(c) => {
                out.cols.insert(*c);
            }
            ScalarKind::Opaque { .. } => out.opaque = true,
            ScalarKind::Subquery {
                rel, correlated, ..
            } => {
                out.cols.extend(correlated.iter().copied());
                out.opaque |= !is_closed(mir, *rel);
            }
            _ => {}
        }
        stack.extend(node.kind.children());
    }
    out
}

/// An `Expr`, by id, without panicking on a bad one.
pub(crate) fn expr(hir: &Hir, id: ExprId) -> Option<&Expr> {
    hir.exprs.get(id)
}

/// The base FROM items under `from`, through nested joins.
pub(crate) fn from_leaves(hir: &Hir, from: FromId) -> BTreeSet<FromId> {
    let mut leaves = BTreeSet::new();
    let mut stack = vec![from];
    let mut budget = hir.from.len() + 1;
    while let Some(id) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        match hir.from.get(id).map(|f| &f.kind) {
            Some(FromKind::Join { l, r, .. }) => {
                stack.push(*l);
                stack.push(*r);
            }
            Some(_) => {
                leaves.insert(id);
            }
            None => {}
        }
    }
    leaves
}

/// The conjuncts of a WHERE or ON tree, left to right.
pub(crate) fn and_chain(hir: &Hir, root: ExprId) -> Vec<ExprId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        match expr(hir, id).map(|e| &e.kind) {
            Some(ExprKind::Binary {
                op: cogitator_ir::common::BinaryOp::And,
                l,
                r,
            }) => {
                stack.push(*r);
                stack.push(*l);
            }
            Some(_) => out.push(id),
            None => {}
        }
    }
    out
}
