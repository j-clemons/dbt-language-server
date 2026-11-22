//! Moves predicates towards the scans (DuckDB `filter_pushdown.cpp`,
//! DataFusion `push_down_filter.rs`, both pinned in specs/ir-pipeline/05).
//!
//! `push` returns the node with what it could place and the predicates
//! that must stay above it. Each case states why the move is safe. Every
//! operator not named here (Limit, SetOp, Unnest, CteRef, Opaque, Scan)
//! keeps its predicates above it.

use std::collections::{BTreeMap, BTreeSet};

use cogitator_ir::mir::{ColId, Mir, ProvId, RelId, RelOp, ScalarId, ScalarKind, WinCall};

use super::join_sides::{where_sides, Sides};
use super::pred::{self, Pred};
use super::rebuild::Rebuilder;

/// Bounds recursion on deep plans. Later passes carry on from where a
/// filter stopped, so this limits one pass's reach, not the result.
const MAX_DEPTH: u32 = 64;

pub(super) struct Pushdown<'a> {
    pub rb: &'a mut Rebuilder,
}

impl Pushdown<'_> {
    pub(super) fn push(
        &mut self,
        mir: &mut Mir,
        node: RelId,
        preds: Vec<Pred>,
        depth: u32,
    ) -> (RelId, Vec<Pred>) {
        if preds.is_empty() || depth >= MAX_DEPTH {
            return (node, preds);
        }
        let Some(rel) = mir.rels.get(node) else {
            return (node, preds);
        };
        let prov = rel.prov;
        // boundary: the op's parts feed nodes appended below
        let op = rel.op.clone();
        let next = depth + 1;
        match op {
            RelOp::Filter { input, conjuncts } => {
                self.merge_filter(mir, node, input, conjuncts, preds, next)
            }
            RelOp::Project { input, items } => {
                let movable = |mir: &Mir, p: &Pred| {
                    p.cols.iter().all(|c| {
                        items
                            .iter()
                            .any(|(col, s)| col == c && pred::is_movable(mir, *s))
                    })
                };
                let map = items.iter().copied().collect();
                let (below, stay) = self.substitute_all(mir, preds, &map, movable);
                if below.is_empty() {
                    return (node, stay);
                }
                let input = self.push_and_wrap(mir, input, below, next);
                let op = RelOp::Project { input, items };
                (self.append(mir, op, prov), stay)
            }
            RelOp::Join { kind, l, r, on } => {
                let (l, r, stay) = self.split_join(mir, l, r, preds, where_sides(kind), next);
                if l.moved || r.moved {
                    let op = RelOp::Join {
                        kind,
                        l: l.node,
                        r: r.node,
                        on,
                    };
                    (self.append(mir, op, prov), stay)
                } else {
                    (node, stay)
                }
            }
            RelOp::Aggregate {
                input,
                keys,
                aggs,
                sets,
            } => {
                // Above grouping, only a key column is the same value
                // below it. A grouping set nulls keys it omits, so the
                // column must be in all of them. A filter with no
                // columns stays: an empty input still gives a global
                // aggregate its one row.
                let keep = |c: &ColId| {
                    sets.as_ref()
                        .is_none_or(|s| s.0.iter().all(|set| set.contains(c)))
                };
                let movable = |mir: &Mir, p: &Pred| {
                    !p.cols.is_empty()
                        && p.cols.iter().all(|c| {
                            keep(c)
                                && keys
                                    .iter()
                                    .any(|(col, s)| col == c && pred::is_movable(mir, *s))
                        })
                };
                let map = keys.iter().copied().collect();
                let (below, stay) = self.substitute_all(mir, preds, &map, movable);
                if below.is_empty() {
                    return (node, stay);
                }
                let input = self.push_and_wrap(mir, input, below, next);
                let op = RelOp::Aggregate {
                    input,
                    keys,
                    aggs,
                    sets,
                };
                (self.append(mir, op, prov), stay)
            }
            RelOp::Window { input, funcs } => {
                // Removing a whole partition leaves the other partitions'
                // results alone, so only partition keys common to every
                // window function may move.
                let common = common_partition_keys(mir, &funcs);
                let (below, stay): (Vec<Pred>, Vec<Pred>) = preds
                    .into_iter()
                    .partition(|p| p.cols.iter().all(|c| common.contains(c)));
                if below.is_empty() {
                    return (node, stay);
                }
                let input = self.push_and_wrap(mir, input, below, next);
                let op = RelOp::Window { input, funcs };
                (self.append(mir, op, prov), stay)
            }
            // Same columns, and a predicate does not depend on row order.
            RelOp::Distinct(input) => {
                let input = self.push_and_wrap(mir, input, preds, next);
                (self.append(mir, RelOp::Distinct(input), prov), Vec::new())
            }
            RelOp::Sort { input, keys } => {
                let input = self.push_and_wrap(mir, input, preds, next);
                let op = RelOp::Sort { input, keys };
                (self.append(mir, op, prov), Vec::new())
            }
            _ => (node, preds),
        }
    }

    /// Merges into a Filter already there. A volatile conjunct in it
    /// must keep seeing the same rows, so nothing moves past it.
    fn merge_filter(
        &mut self,
        mir: &mut Mir,
        node: RelId,
        input: RelId,
        mut conjuncts: Vec<ScalarId>,
        preds: Vec<Pred>,
        next: u32,
    ) -> (RelId, Vec<Pred>) {
        if !conjuncts.iter().all(|c| pred::is_movable(mir, *c)) {
            return (node, preds);
        }
        let prov = mir.rels.get(node).map(|r| r.prov);
        let (below, rest) = self.push(mir, input, preds, next);
        conjuncts.extend(rest.iter().map(|p| p.id));
        let Some(prov) = prov else {
            return (node, rest);
        };
        let op = RelOp::Filter {
            input: below,
            conjuncts,
        };
        (self.append(mir, op, prov), Vec::new())
    }

    /// Sends each predicate into the input whose columns cover it.
    pub(super) fn split_join(
        &mut self,
        mir: &mut Mir,
        l: RelId,
        r: RelId,
        preds: Vec<Pred>,
        sides: Sides,
        next: u32,
    ) -> (Placed, Placed, Vec<Pred>) {
        let columns = |mir: &Mir, id: RelId| -> BTreeSet<ColId> {
            mir.rels
                .get(id)
                .map_or_else(BTreeSet::new, |r| r.out.iter().copied().collect())
        };
        let (left_cols, right_cols) = (columns(mir, l), columns(mir, r));
        let (mut left, mut right, mut stay) = (Vec::new(), Vec::new(), Vec::new());
        for p in preds {
            if sides.left && p.cols.is_subset(&left_cols) {
                left.push(p);
            } else if sides.right && p.cols.is_subset(&right_cols) {
                right.push(p);
            } else {
                stay.push(p);
            }
        }
        let place = |this: &mut Self, mir: &mut Mir, id: RelId, preds: Vec<Pred>| {
            let moved = !preds.is_empty();
            Placed {
                node: this.push_and_wrap(mir, id, preds, next),
                moved,
            }
        };
        let left = place(self, mir, l, left);
        let right = place(self, mir, r, right);
        (left, right, stay)
    }

    fn substitute_all(
        &mut self,
        mir: &mut Mir,
        preds: Vec<Pred>,
        map: &BTreeMap<ColId, ScalarId>,
        movable: impl Fn(&Mir, &Pred) -> bool,
    ) -> (Vec<Pred>, Vec<Pred>) {
        let (mut below, mut stay) = (Vec::new(), Vec::new());
        for p in preds {
            let substituted = if movable(mir, &p) {
                pred::substitute(self.rb, mir, &p, map)
            } else {
                None
            };
            match substituted {
                Some(new) => below.push(new),
                None => stay.push(p),
            }
        }
        (below, stay)
    }

    /// Pushes as far as possible, then filters on whatever is left.
    fn push_and_wrap(&mut self, mir: &mut Mir, input: RelId, preds: Vec<Pred>, next: u32) -> RelId {
        let (below, rest) = self.push(mir, input, preds, next);
        let Some(origin) = rest
            .first()
            .and_then(|p| mir.scalars.get(p.id))
            .map(|s| s.prov)
        else {
            return below;
        };
        let conjuncts = rest.iter().map(|p| p.id).collect();
        let op = RelOp::Filter {
            input: below,
            conjuncts,
        };
        self.append(mir, op, origin)
    }

    /// A new node whose origin is `from`, so a moved filter's span is
    /// still the clause the user wrote.
    fn append(&mut self, mir: &mut Mir, op: RelOp, from: ProvId) -> RelId {
        let prov = self.rb.derive(mir, from);
        mir.add_rel(op, prov)
    }
}

/// One join input after placement.
#[derive(Debug, Clone, Copy)]
pub(super) struct Placed {
    pub node: RelId,
    pub moved: bool,
}

/// Columns every window function partitions by directly. A computed
/// partition key is not a column a predicate can name.
fn common_partition_keys(mir: &Mir, funcs: &[(ColId, WinCall)]) -> BTreeSet<ColId> {
    let bare_keys = |call: &WinCall| -> BTreeSet<ColId> {
        call.partition_by
            .iter()
            .filter_map(|s| match mir.scalars.get(*s).map(|n| &n.kind) {
                Some(ScalarKind::Col(c)) => Some(*c),
                _ => None,
            })
            .collect()
    };
    let mut calls = funcs.iter().map(|(_, call)| bare_keys(call));
    let Some(first) = calls.next() else {
        return BTreeSet::new();
    };
    calls.fold(first, |acc, keys| {
        acc.intersection(&keys).copied().collect()
    })
}
