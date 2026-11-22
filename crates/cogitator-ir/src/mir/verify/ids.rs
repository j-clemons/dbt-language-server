//! Ids in range, and every operand defined before its user.

use crate::mir::{Mir, RelOp, ScalarKind};
use crate::{Arena, Idx, Invariant, Violation};

struct Ids<'a> {
    out: &'a mut Vec<Violation>,
}

impl Ids<'_> {
    fn id<T>(&mut self, arena: &Arena<T>, id: Idx<T>, what: &str) {
        if !arena.contains(id) {
            self.out.push(Violation::new(
                Invariant::MirIdRange,
                format!("{what} {id:?} is out of range ({})", arena.len()),
            ));
        }
    }

    fn precedes<T>(&mut self, operand: Idx<T>, user: Idx<T>, what: &str) {
        if operand >= user {
            self.out.push(Violation::new(
                Invariant::MirIdRange,
                format!("{what} {operand:?} does not precede its user {user:?}"),
            ));
        }
    }
}

pub(super) fn check(mir: &Mir, out: &mut Vec<Violation>) {
    let mut c = Ids { out };
    c.id(&mir.rels, mir.root, "root");
    for (_, cte) in mir.ctes.iter() {
        c.id(&mir.rels, cte.body, "cte body");
        for col in &cte.out {
            c.id(&mir.cols, *col, "cte output");
        }
    }
    for (id, scalar) in mir.scalars.iter() {
        for child in scalar.kind.children() {
            c.id(&mir.scalars, child, "scalar child");
            c.precedes(child, id, "scalar child");
        }
        match &scalar.kind {
            ScalarKind::Col(col) | ScalarKind::Outer(col) => c.id(&mir.cols, *col, "scalar col"),
            ScalarKind::Subquery {
                rel, correlated, ..
            } => {
                c.id(&mir.rels, *rel, "subquery rel");
                for col in correlated {
                    c.id(&mir.cols, *col, "correlated col");
                }
            }
            _ => {}
        }
    }
    for (id, rel) in mir.rels.iter() {
        for input in rel.op.inputs() {
            c.id(&mir.rels, input, "input");
            c.precedes(input, id, "input");
        }
        for scalar in rel.op.scalars() {
            c.id(&mir.scalars, scalar, "rel scalar");
        }
        for col in rel.out.iter().chain(defined_cols(&rel.op).iter()) {
            c.id(&mir.cols, *col, "rel column");
        }
        match &rel.op {
            RelOp::Scan { table, .. } => c.id(&mir.tables, *table, "scan table"),
            RelOp::CteRef { cte, .. } => c.id(&mir.ctes, *cte, "cte ref"),
            _ => {}
        }
    }
    if c.out.is_empty() {
        check_subquery_order(mir, &mut c);
    }
}

/// A subquery's relation must precede the relation evaluating it; the
/// bottom-up correlation check relies on this.
fn check_subquery_order(mir: &Mir, c: &mut Ids<'_>) {
    for (id, rel) in mir.rels.iter() {
        let mut stack = rel.op.scalars();
        let mut guard = 0;
        while let Some(s) = stack.pop() {
            guard += 1;
            if guard > mir.scalars.len() {
                break;
            }
            let Some(scalar) = mir.scalars.get(s) else {
                continue;
            };
            if let ScalarKind::Subquery { rel: sub, .. } = scalar.kind {
                c.precedes(sub, id, "subquery rel");
            }
            stack.extend(scalar.kind.children());
        }
    }
}

/// Column ids named by the op itself (not just its `out`).
fn defined_cols(op: &RelOp) -> Vec<crate::mir::ColId> {
    match op {
        RelOp::Scan { cols, .. } => cols.clone(),
        RelOp::Project { items, .. } => items.iter().map(|(c, _)| *c).collect(),
        RelOp::Aggregate { keys, aggs, .. } => keys
            .iter()
            .map(|(c, _)| *c)
            .chain(aggs.iter().map(|(c, _)| *c))
            .collect(),
        RelOp::Window { funcs, .. } => funcs.iter().map(|(c, _)| *c).collect(),
        RelOp::SetOp { out, .. }
        | RelOp::Unnest { out, .. }
        | RelOp::CteRef { out, .. }
        | RelOp::Opaque { out, .. } => out.clone(),
        RelOp::Filter { .. }
        | RelOp::Join { .. }
        | RelOp::Distinct(_)
        | RelOp::Sort { .. }
        | RelOp::Limit { .. } => Vec::new(),
    }
}
