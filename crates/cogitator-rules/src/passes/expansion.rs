//! Plan size, including how large the plan becomes when walked.
//!
//! The arenas can hold a DAG: a scalar may share a child, so `x = y + y`
//! repeated 64 times is 64 nodes that walk as 2^64. The verifier, the
//! printer and `Mir::reachable` all walk trees without deduplicating, so
//! the size that matters is the expanded one. It is computed here in one
//! linear pass over the arenas, before any of those walks can start.
//! Operands precede users in verified IR; an operand that does not
//! counts as a leaf, and the verifier then reports it.

use cogitator_ir::mir::{Mir, RelId, RelOp, ScalarId, ScalarKind};

/// The most nodes a plan may expand to and still be walked. Above the
/// front end's 200k node cap, so a tree-shaped plan always fits; only
/// sharing can exceed it.
pub(super) const MAX_EXPANDED: u64 = 1 << 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Size {
    /// Allocated relations and scalars, including superseded nodes.
    pub arena: u64,
    /// Nodes a tree walk of the reachable plan visits, saturating.
    pub expanded: u64,
}

impl Size {
    /// The figure charged against `Budgets::max_nodes`.
    pub fn charged(self) -> u32 {
        u32::try_from(self.arena.max(self.expanded)).unwrap_or(u32::MAX)
    }

    pub fn walkable(self) -> bool {
        self.expanded <= MAX_EXPANDED
    }
}

pub(super) fn measure(mir: &Mir) -> Size {
    let scalars = scalar_weights(mir);
    let rels = rel_weights(mir, &scalars);
    let at = |id: RelId| rels.get(id.index()).copied().unwrap_or(1);

    let mut walk = Walk::new(mir);
    walk.rels.push(mir.root);
    let mut expanded = at(mir.root);
    while let Some(id) = walk.next_rel() {
        let Some(rel) = mir.rels.get(id) else {
            continue;
        };
        walk.rels.extend(rel.op.inputs());
        if let RelOp::CteRef { cte, .. } = rel.op {
            if let Some(def) = mir.ctes.get(cte) {
                if !walk.rel_seen(def.body) {
                    expanded = expanded.saturating_add(at(def.body));
                }
                walk.rels.push(def.body);
            }
        }
        for root in rel.op.scalars() {
            expanded = expanded.saturating_add(weight(&scalars, root));
            walk.scalar_tree(root);
        }
    }
    Size {
        arena: (mir.rels.len() as u64).saturating_add(mir.scalars.len() as u64),
        expanded,
    }
}

fn weight(weights: &[u64], id: ScalarId) -> u64 {
    weights.get(id.index()).copied().unwrap_or(1)
}

fn scalar_weights(mir: &Mir) -> Vec<u64> {
    let mut weights: Vec<u64> = Vec::with_capacity(mir.scalars.len());
    for (id, scalar) in mir.scalars.iter() {
        let total = scalar
            .kind
            .children()
            .into_iter()
            .filter(|child| *child < id)
            .fold(1u64, |sum, child| {
                sum.saturating_add(weight(&weights, child))
            });
        weights.push(total);
    }
    weights
}

/// Per-relation size of the tree the printer expands from that relation.
fn rel_weights(mir: &Mir, scalars: &[u64]) -> Vec<u64> {
    let mut weights: Vec<u64> = Vec::with_capacity(mir.rels.len());
    for (id, rel) in mir.rels.iter() {
        let own = rel
            .op
            .scalars()
            .into_iter()
            .fold(1u64, |sum, s| sum.saturating_add(weight(scalars, s)));
        let total = rel
            .op
            .inputs()
            .into_iter()
            .filter(|input| *input < id)
            .fold(own, |sum, input| {
                sum.saturating_add(weights.get(input.index()).copied().unwrap_or(1))
            });
        weights.push(total);
    }
    weights
}

/// A deduplicating walk, so measuring stays linear on shared nodes.
struct Walk<'a> {
    mir: &'a Mir,
    rels: Vec<RelId>,
    seen_rels: Vec<bool>,
    seen_scalars: Vec<bool>,
}

impl<'a> Walk<'a> {
    fn new(mir: &'a Mir) -> Self {
        Self {
            mir,
            rels: Vec::new(),
            seen_rels: vec![false; mir.rels.len()],
            seen_scalars: vec![false; mir.scalars.len()],
        }
    }

    fn rel_seen(&self, id: RelId) -> bool {
        self.seen_rels.get(id.index()).copied().unwrap_or(true)
    }

    fn next_rel(&mut self) -> Option<RelId> {
        while let Some(id) = self.rels.pop() {
            if let Some(slot) = self.seen_rels.get_mut(id.index()) {
                if !std::mem::replace(slot, true) {
                    return Some(id);
                }
            }
        }
        None
    }

    /// Queues the relations of subqueries under `root`.
    fn scalar_tree(&mut self, root: ScalarId) {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(slot) = self.seen_scalars.get_mut(id.index()) else {
                continue;
            };
            if std::mem::replace(slot, true) {
                continue;
            }
            let Some(scalar) = self.mir.scalars.get(id) else {
                continue;
            };
            if let ScalarKind::Subquery { rel, .. } = scalar.kind {
                self.rels.push(rel);
            }
            stack.extend(scalar.kind.children());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cogitator_ir::common::BinaryOp;
    use cogitator_ir::testkit::{int_col, MirBuilder};

    #[test]
    fn a_tree_expands_to_its_own_size() {
        let mut b = MirBuilder::new();
        let scan = b.scan("t", &[("a", int_col())]);
        let a = b.out(scan)[0];
        let r = b.col_ref(a);
        let one = b.number("1");
        let pred = b.binary(BinaryOp::Eq, r, one);
        let filter = b.filter(scan, vec![pred]);
        let size = measure(&b.finish(filter));
        assert_eq!(size.arena, 5);
        // filter tree (2 rels + 3 scalars) plus the verifier's scalar walk
        assert_eq!(size.expanded, 8);
    }

    #[test]
    fn shared_scalars_are_measured_without_walking_them() {
        let mut b = MirBuilder::new();
        let scan = b.scan("t", &[("a", int_col())]);
        let a = b.out(scan)[0];
        let mut s = b.col_ref(a);
        for _ in 0..128 {
            s = b.binary(BinaryOp::Plus, s, s);
        }
        let filter = b.filter(scan, vec![s]);
        let size = measure(&b.finish(filter));
        assert_eq!(size.arena, 131);
        assert_eq!(size.expanded, u64::MAX);
        assert!(!size.walkable());
    }

    #[test]
    fn bad_ids_do_not_panic() {
        let mut b = MirBuilder::new();
        let scan = b.scan("t", &[("a", int_col())]);
        let mut mir = b.finish(scan);
        mir.root = RelId::from_raw(99);
        assert_eq!(measure(&mir).arena, 1);
    }
}
