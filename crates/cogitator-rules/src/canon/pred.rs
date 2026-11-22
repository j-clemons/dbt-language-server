//! What a pushdown needs to know about one predicate: which columns it
//! reads, whether moving it is safe, and how to rewrite it in terms of
//! the columns below a Project or Aggregate.

use std::collections::{BTreeMap, BTreeSet};

use cogitator_ir::common::{BinaryOp, UnaryOp};
use cogitator_ir::mir::{ColId, Mir, ScalarId, ScalarKind};

use super::rebuild::Rebuilder;
use super::scalar_map::map_children;

/// Past this many nodes a predicate is not inspected or substituted.
/// This keeps a substitution from multiplying a shared expression.
const MAX_PRED_NODES: usize = 512;

/// Functions known to be deterministic and row-local. Anything else
/// (volatile, session-dependent, or simply unknown, such as a UDF) pins
/// its predicate in place.
const DETERMINISTIC: &[&str] = &[
    "abs",
    "ceil",
    "ceiling",
    "coalesce",
    "concat",
    "date",
    "date_trunc",
    "day",
    "floor",
    "if",
    "ifnull",
    "length",
    "lower",
    "ltrim",
    "month",
    "nullif",
    "nvl",
    "replace",
    "round",
    "rtrim",
    "substr",
    "substring",
    "trim",
    "upper",
    "year",
];

/// A conjunct that is safe to move, with the columns it reads.
#[derive(Debug, Clone)]
pub(super) struct Pred {
    pub id: ScalarId,
    pub cols: BTreeSet<ColId>,
}

/// `Some` when `id` may be moved. Subqueries and opaque holes never move,
/// nor does anything calling an unknown function or operator.
pub(super) fn inspect(mir: &Mir, id: ScalarId) -> Option<Pred> {
    let mut cols = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![id];
    while let Some(at) = stack.pop() {
        if !seen.insert(at) {
            continue;
        }
        if seen.len() > MAX_PRED_NODES {
            return None;
        }
        let node = &mir.scalars.get(at)?.kind;
        if !safe_node(mir, node) {
            return None;
        }
        if let ScalarKind::Col(col) = node {
            cols.insert(*col);
        }
        stack.extend(node.children());
    }
    Some(Pred { id, cols })
}

/// Whether the expression could be moved, for Project and Aggregate items.
pub(super) fn is_movable(mir: &Mir, id: ScalarId) -> bool {
    inspect(mir, id).is_some()
}

fn safe_node(mir: &Mir, node: &ScalarKind) -> bool {
    match node {
        ScalarKind::Subquery { .. }
        | ScalarKind::Opaque { .. }
        | ScalarKind::Unary {
            op: UnaryOp::Other(_),
            ..
        }
        | ScalarKind::Binary {
            op: BinaryOp::Other(_),
            ..
        } => false,
        ScalarKind::Call { name, .. } => {
            let text = mir.syms.text(*name);
            DETERMINISTIC.iter().any(|f| f.eq_ignore_ascii_case(text))
        }
        _ => true,
    }
}

/// The conjuncts of `roots` with top-level ANDs flattened, in order.
pub(super) fn split_conjuncts(mir: &Mir, roots: &[ScalarId]) -> Vec<ScalarId> {
    let mut out = Vec::with_capacity(roots.len());
    let mut stack: Vec<ScalarId> = roots.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        match mir.scalars.get(id).map(|s| &s.kind) {
            Some(ScalarKind::Binary {
                op: BinaryOp::And,
                l,
                r,
            }) => {
                stack.push(*r);
                stack.push(*l);
            }
            _ => out.push(id),
        }
    }
    out
}

/// Rewrites `pred` in terms of the columns below: `Col(c)` becomes
/// `map[&c]`. Subtrees are shared rather than copied, so the arena grows
/// only along the rewritten path. `None` if the result would be too big.
pub(super) fn substitute(
    rb: &mut Rebuilder,
    mir: &mut Mir,
    pred: &Pred,
    map: &BTreeMap<ColId, ScalarId>,
) -> Option<Pred> {
    if expanded_size(mir, pred.id, map)? > MAX_PRED_NODES {
        return None;
    }
    let mut memo: BTreeMap<ScalarId, ScalarId> = BTreeMap::new();
    let mut order = BTreeSet::new();
    let mut stack = vec![pred.id];
    while let Some(id) = stack.pop() {
        if order.insert(id) {
            stack.extend(mir.scalars.get(id)?.kind.children());
        }
    }
    // ascending: children precede their users
    for id in order {
        let node = mir.scalars.get(id)?;
        let original = node.kind.clone();
        let replaced = match original {
            ScalarKind::Col(c) => map.get(&c).copied(),
            _ => None,
        };
        let new = match replaced {
            Some(target) => target,
            None => {
                let kind = map_children(
                    original.clone(),
                    &|c| memo.get(&c).copied().unwrap_or(c),
                    &|r| r,
                );
                if kind == original {
                    id
                } else {
                    let (ty, from) = (node.ty.clone(), node.prov);
                    let prov = rb.derive(mir, from);
                    mir.add_scalar(kind, ty, prov)
                }
            }
        };
        memo.insert(id, new);
    }
    let id = memo.get(&pred.id).copied()?;
    let mut cols = BTreeSet::new();
    let mut stack = vec![id];
    let mut seen = BTreeSet::new();
    while let Some(at) = stack.pop() {
        if !seen.insert(at) {
            continue;
        }
        let node = &mir.scalars.get(at)?.kind;
        if let ScalarKind::Col(c) = node {
            cols.insert(*c);
        }
        stack.extend(node.children());
    }
    Some(Pred { id, cols })
}

/// The node count of `pred` after substitution, walking the expanded
/// tree but stopping as soon as it passes the cap. A substituted
/// expression is counted as written: its columns belong to the input
/// below, so the map does not apply to them again.
fn expanded_size(mir: &Mir, root: ScalarId, map: &BTreeMap<ColId, ScalarId>) -> Option<usize> {
    let mut total = 0usize;
    let mut stack = vec![(root, true)];
    while let Some((id, substituting)) = stack.pop() {
        total += 1;
        if total > MAX_PRED_NODES {
            return Some(total);
        }
        let node = &mir.scalars.get(id)?.kind;
        match node {
            ScalarKind::Col(c) if substituting => {
                if let Some(target) = map.get(c) {
                    stack.push((*target, false));
                }
            }
            _ => stack.extend(node.children().into_iter().map(|c| (c, substituting))),
        }
    }
    Some(total)
}
