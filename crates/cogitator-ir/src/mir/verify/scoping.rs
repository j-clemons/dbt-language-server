//! ColId definition and visibility, including correlation. Runs after
//! `ids`, so indexing cannot miss and operands precede users.

use std::collections::BTreeSet;

use crate::mir::{ColId, Mir, RelId, RelOp, ScalarId, ScalarKind};
use crate::{Invariant, Violation};

pub(super) fn for_scalar_tree(mir: &Mir, root: ScalarId, mut f: impl FnMut(ScalarId)) {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        f(id);
        stack.extend(mir.scalars[id].kind.children());
    }
}

pub(super) fn subquery_rels(mir: &Mir, root: ScalarId, stack: &mut Vec<RelId>) {
    for_scalar_tree(mir, root, |id| {
        if let ScalarKind::Subquery { rel, .. } = mir.scalars[id].kind {
            stack.push(rel);
        }
    });
}

pub(super) fn defined_once(mir: &Mir, reachable: &[RelId], out: &mut Vec<Violation>) {
    let mut defined_by: Vec<Option<RelId>> = vec![None; mir.cols.len()];
    for &id in reachable {
        for col in mir.rels[id].op.defined(&mir.scalars) {
            let slot = &mut defined_by[col.index()];
            if let Some(prev) = *slot {
                out.push(Violation::new(
                    Invariant::MirColDefinedOnce,
                    format!("column {col:?} defined by {prev:?} and {id:?}"),
                ));
            }
            *slot = Some(id);
        }
    }
}

/// Every `Col` is visible from the node's inputs, every `Outer` is
/// declared by the enclosing subquery, and no correlation escapes the
/// plan root or a CTE body.
pub(super) fn visibility(mir: &Mir, reachable: &[RelId], out: &mut Vec<Violation>) {
    let bad = |detail: String| Violation::new(Invariant::MirColVisible, detail);
    let mut free: Vec<BTreeSet<ColId>> = vec![BTreeSet::new(); mir.rels.len()];
    for &id in reachable {
        let rel = &mir.rels[id];
        let inputs = rel.op.inputs();
        let visible: BTreeSet<ColId> = inputs
            .iter()
            .flat_map(|input| mir.rels[*input].out.iter().copied())
            .collect();
        let mut own_free: BTreeSet<ColId> = inputs
            .iter()
            .flat_map(|input| free[input.index()].iter().copied())
            .collect();
        for root in rel.op.scalars() {
            for_scalar_tree(mir, root, |sid| match &mir.scalars[sid].kind {
                ScalarKind::Col(col) if !visible.contains(col) => {
                    out.push(bad(format!(
                        "rel {id:?} reads {col:?} which is not visible"
                    )));
                }
                ScalarKind::Outer(col) => {
                    own_free.insert(*col);
                }
                ScalarKind::Subquery {
                    rel: sub,
                    correlated,
                    ..
                } => {
                    let inner = &free[sub.index()];
                    for col in inner {
                        if !correlated.contains(col) {
                            out.push(bad(format!(
                                "subquery {sub:?} reads outer {col:?} it does not declare"
                            )));
                        }
                    }
                    own_free.extend(correlated.iter().filter(|c| !visible.contains(*c)).copied());
                }
                _ => {}
            });
        }
        free[id.index()] = own_free;
    }
    for col in &free[mir.root.index()] {
        out.push(bad(format!("plan root leaves correlated {col:?} unbound")));
    }
    for cte in mir.ctes.values() {
        if reachable.contains(&cte.body) {
            for col in &free[cte.body.index()] {
                out.push(bad(format!("cte body leaves correlated {col:?} unbound")));
            }
        }
    }
}

/// A column reference may only claim a base type as specific as the
/// column's definition. Nullability is exempt: outer joins widen it
/// above the definition.
pub(super) fn column_types(mir: &Mir, reachable: &[RelId], out: &mut Vec<Violation>) {
    for &id in reachable {
        let mut used = mir.rels[id].op.scalars();
        while let Some(sid) = used.pop() {
            let scalar = &mir.scalars[sid];
            if let ScalarKind::Col(col) | ScalarKind::Outer(col) = scalar.kind {
                let declared = &mir.cols[col].ty.ty;
                if !scalar.ty.ty.refines(declared) {
                    out.push(Violation::new(
                        Invariant::MirTypeSpecificity,
                        format!(
                            "{col:?} declared {declared:?}, referenced as {:?}",
                            scalar.ty.ty
                        ),
                    ));
                }
            }
            used.extend(scalar.kind.children());
        }
        if let RelOp::CteRef { cte, out: refs } = &mir.rels[id].op {
            let declared = mir.ctes[*cte].out.iter().map(|c| &mir.cols[*c].ty);
            let actual = refs.iter().map(|c| &mir.cols[*c].ty);
            for (d, a) in declared.zip(actual) {
                if !a.refines(d) {
                    out.push(Violation::new(
                        Invariant::MirTypeSpecificity,
                        format!("cte ref {id:?} widens {d:?} to {a:?}"),
                    ));
                }
            }
        }
    }
}
