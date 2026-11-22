//! The shared rewrite driver for canonicalisers.
//!
//! Visits the reachable plan bottom-up, in ascending id order so operands
//! are rewritten before their users. Nothing is mutated: a node that
//! changes is appended, and every user is rebuilt to point at it, up to
//! a new `root`. A pass that changes nothing appends nothing.

use std::collections::{BTreeMap, BTreeSet};

use cogitator_ir::common::Lit;
use cogitator_ir::mir::{
    ColId, CteDef, CteId, Mir, Origin, ProvId, RelId, RelOp, ScalarId, ScalarKind,
};
use cogitator_ir::RuleId;

use super::scalar_map::{for_each_scalar, map_children};
use crate::rule::Rewrite;

/// What a canonicaliser does with one scalar, whose children are already
/// rewritten.
pub(super) enum Fold {
    Keep(ScalarKind),
    /// Becomes this literal, keeping the node's type and origin.
    Lit(Lit),
    /// Becomes an existing scalar.
    Replace(ScalarId),
}

pub(super) fn no_fold(_: &Mir, kind: ScalarKind) -> Fold {
    Fold::Keep(kind)
}

pub(super) fn run<F, S>(mut mir: Mir, rule: RuleId, mut fold: F, mut step: S) -> Rewrite<Mir>
where
    F: FnMut(&Mir, ScalarKind) -> Fold,
    S: FnMut(&mut Rebuilder, &mut Mir, RelId, RelOp) -> RelId,
{
    let mut rb = Rebuilder::new(&mir, rule);
    for old in mir.reachable() {
        let Some(rel) = mir.rels.get(old) else {
            continue;
        };
        // boundary: a rewritten node owns its op (append-only arena)
        let mut op = rel.op.clone();
        rb.map_refs(&mut mir, &mut op);
        rb.map_scalars(&mut mir, &mut op, &mut fold);
        let new = step(&mut rb, &mut mir, old, op);
        if let Some(slot) = rb.rels.get_mut(old.index()) {
            *slot = Some(new);
        }
    }
    let root = rb.rel(mir.root);
    if root == mir.root {
        Rewrite::Unchanged(mir)
    } else {
        mir.root = root;
        Rewrite::Changed(mir)
    }
}

pub(super) struct Rebuilder {
    rule: RuleId,
    rels: Vec<Option<RelId>>,
    scalars: Vec<Option<ScalarId>>,
    ctes: Vec<Option<CteId>>,
    derived: BTreeMap<ProvId, ProvId>,
}

impl Rebuilder {
    fn new(mir: &Mir, rule: RuleId) -> Self {
        Self {
            rule,
            rels: vec![None; mir.rels.len()],
            scalars: vec![None; mir.scalars.len()],
            ctes: vec![None; mir.ctes.len()],
            derived: BTreeMap::new(),
        }
    }

    pub(super) fn rel(&self, old: RelId) -> RelId {
        self.rels.get(old.index()).copied().flatten().unwrap_or(old)
    }

    fn mapped_scalar(&self, old: ScalarId) -> ScalarId {
        self.scalars
            .get(old.index())
            .copied()
            .flatten()
            .unwrap_or(old)
    }

    /// Provenance for a node derived from `from`; repeated derivations
    /// from one source share an entry, so the table grows with the plan.
    pub(super) fn derive(&mut self, mir: &mut Mir, from: ProvId) -> ProvId {
        if let Some(done) = self.derived.get(&from) {
            return *done;
        }
        let new = mir.prov.add(Origin::Derived {
            rule: self.rule,
            from: vec![from],
        });
        self.derived.insert(from, new);
        new
    }

    /// `op` as a node, reusing `old` when nothing changed.
    pub(super) fn emit(&mut self, mir: &mut Mir, old: RelId, op: RelOp) -> RelId {
        let Some(rel) = mir.rels.get(old) else {
            return old;
        };
        if rel.op == op {
            return old;
        }
        let from = rel.prov;
        let prov = self.derive(mir, from);
        mir.add_rel(op, prov)
    }

    fn map_refs(&mut self, mir: &mut Mir, op: &mut RelOp) {
        match op {
            RelOp::CteRef { cte, out } => self.map_cte_ref(mir, cte, out),
            RelOp::Scan { .. } => {}
            RelOp::Filter { input, .. }
            | RelOp::Project { input, .. }
            | RelOp::Aggregate { input, .. }
            | RelOp::Window { input, .. }
            | RelOp::Distinct(input)
            | RelOp::Sort { input, .. }
            | RelOp::Limit { input, .. }
            | RelOp::Unnest { input, .. } => *input = self.rel(*input),
            RelOp::Join { l, r, .. } => {
                *l = self.rel(*l);
                *r = self.rel(*r);
            }
            RelOp::SetOp { inputs, .. } | RelOp::Opaque { inputs, .. } => {
                for input in inputs {
                    *input = self.rel(*input);
                }
            }
        }
    }

    /// Repoints a reference at the rewritten CTE, keeping the columns the
    /// new body still produces, by position.
    fn map_cte_ref(&mut self, mir: &mut Mir, cte: &mut CteId, out: &mut Vec<ColId>) {
        let new = self.cte(mir, *cte);
        if new == *cte {
            return;
        }
        if let (Some(old_def), Some(new_def)) = (mir.ctes.get(*cte), mir.ctes.get(new)) {
            if old_def.out.len() == out.len() {
                let kept: BTreeSet<ColId> = new_def.out.iter().copied().collect();
                let mut position = old_def.out.iter();
                out.retain(|_| position.next().is_some_and(|c| kept.contains(c)));
            }
        }
        *cte = new;
    }

    /// The CTE to reference after rewriting. Appends a definition when
    /// the body changed. A body that follows its reference in id order
    /// is not rewritten yet, and the reference keeps the original.
    fn cte(&mut self, mir: &mut Mir, old: CteId) -> CteId {
        if let Some(Some(done)) = self.ctes.get(old.index()) {
            return *done;
        }
        let Some(def) = mir.ctes.get(old) else {
            return old;
        };
        let body = self.rel(def.body);
        let new = if body == def.body {
            old
        } else {
            let name = def.name;
            let out = mir.rels.get(body).map_or(Vec::new(), |r| r.out.clone());
            mir.ctes.alloc(CteDef { name, body, out })
        };
        if let Some(slot) = self.ctes.get_mut(old.index()) {
            *slot = Some(new);
        }
        new
    }

    fn map_scalars<F>(&mut self, mir: &mut Mir, op: &mut RelOp, fold: &mut F)
    where
        F: FnMut(&Mir, ScalarKind) -> Fold,
    {
        for_each_scalar(op, &mut |s| *s = self.scalar(mir, *s, fold));
    }

    /// Rewrites one scalar tree. Each shared node is done once, so the
    /// cost is the tree's distinct nodes, not its expanded size.
    fn scalar<F>(&mut self, mir: &mut Mir, root: ScalarId, fold: &mut F) -> ScalarId
    where
        F: FnMut(&Mir, ScalarKind) -> Fold,
    {
        let done = |rb: &Self, id: ScalarId| matches!(rb.scalars.get(id.index()), Some(Some(_)));
        let mut order = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if done(self, id) || !order.insert(id) {
                continue;
            }
            if let Some(node) = mir.scalars.get(id) {
                stack.extend(node.kind.children());
            }
        }
        // BTreeSet order is ascending, and children precede their users
        for id in order {
            let new = self.scalar_node(mir, id, fold);
            if let Some(slot) = self.scalars.get_mut(id.index()) {
                *slot = Some(new);
            }
        }
        self.mapped_scalar(root)
    }

    fn scalar_node<F>(&mut self, mir: &mut Mir, id: ScalarId, fold: &mut F) -> ScalarId
    where
        F: FnMut(&Mir, ScalarKind) -> Fold,
    {
        let Some(node) = mir.scalars.get(id) else {
            return id;
        };
        // boundary: `kind` feeds a possibly appended node
        let original = node.kind.clone();
        let mapped = map_children(original.clone(), &|c| self.mapped_scalar(c), &|r| {
            self.rel(r)
        });
        let kind = match fold(mir, mapped) {
            Fold::Replace(existing) => return existing,
            Fold::Keep(kind) if kind == original => return id,
            Fold::Keep(kind) => kind,
            Fold::Lit(lit) => ScalarKind::Lit(lit),
        };
        let Some(node) = mir.scalars.get(id) else {
            return id;
        };
        let (ty, from) = (node.ty.clone(), node.prov);
        let prov = self.derive(mir, from);
        mir.add_scalar(kind, ty, prov)
    }
}
