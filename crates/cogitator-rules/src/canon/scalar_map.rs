//! Mapping over the scalar ids held by a scalar or a relational op.

use cogitator_ir::mir::{RelId, RelOp, ScalarId, ScalarKind, SubqueryKind};

/// `kind` with its child scalars and subquery relation mapped.
pub(super) fn map_children(
    kind: ScalarKind,
    scalar: &impl Fn(ScalarId) -> ScalarId,
    rel: &impl Fn(RelId) -> RelId,
) -> ScalarKind {
    let many = |xs: Vec<ScalarId>| xs.into_iter().map(scalar).collect::<Vec<_>>();
    match kind {
        leaf @ (ScalarKind::Col(_) | ScalarKind::Outer(_) | ScalarKind::Lit(_)) => leaf,
        ScalarKind::Unary { op, e } => ScalarKind::Unary { op, e: scalar(e) },
        ScalarKind::Binary { op, l, r } => ScalarKind::Binary {
            op,
            l: scalar(l),
            r: scalar(r),
        },
        ScalarKind::Call {
            name,
            args,
            distinct,
        } => ScalarKind::Call {
            name,
            args: many(args),
            distinct,
        },
        ScalarKind::Case {
            operand,
            arms,
            else_,
        } => ScalarKind::Case {
            operand: operand.map(scalar),
            arms: arms
                .into_iter()
                .map(|(w, t)| (scalar(w), scalar(t)))
                .collect(),
            else_: else_.map(scalar),
        },
        ScalarKind::Cast { e, ty } => ScalarKind::Cast { e: scalar(e), ty },
        ScalarKind::InList { e, list, negated } => ScalarKind::InList {
            e: scalar(e),
            list: many(list),
            negated,
        },
        ScalarKind::Subquery {
            kind,
            rel: sub,
            correlated,
        } => ScalarKind::Subquery {
            kind: match kind {
                SubqueryKind::In { e, negated } => SubqueryKind::In {
                    e: scalar(e),
                    negated,
                },
                other => other,
            },
            rel: rel(sub),
            correlated,
        },
        ScalarKind::Opaque { children, hole } => ScalarKind::Opaque {
            children: many(children),
            hole,
        },
    }
}

pub(super) fn for_each_scalar(op: &mut RelOp, f: &mut impl FnMut(&mut ScalarId)) {
    match op {
        RelOp::Scan { .. }
        | RelOp::Distinct(_)
        | RelOp::SetOp { .. }
        | RelOp::CteRef { .. }
        | RelOp::Opaque { .. } => {}
        RelOp::Filter { conjuncts: xs, .. }
        | RelOp::Join { on: xs, .. }
        | RelOp::Unnest { exprs: xs, .. } => xs.iter_mut().for_each(f),
        RelOp::Project { items, .. } => items.iter_mut().for_each(|(_, s)| f(s)),
        RelOp::Aggregate { keys, aggs, .. } => {
            keys.iter_mut().for_each(|(_, s)| f(s));
            for (_, agg) in aggs {
                agg.args.iter_mut().for_each(&mut *f);
                agg.filter.iter_mut().for_each(&mut *f);
            }
        }
        RelOp::Window { funcs, .. } => {
            for (_, win) in funcs {
                win.args.iter_mut().for_each(&mut *f);
                win.partition_by.iter_mut().for_each(&mut *f);
                win.order_by.iter_mut().for_each(|k| f(&mut k.expr));
            }
        }
        RelOp::Sort { keys, .. } => keys.iter_mut().for_each(|k| f(&mut k.expr)),
        RelOp::Limit { limit, offset, .. } => {
            limit.iter_mut().for_each(&mut *f);
            offset.iter_mut().for_each(f);
        }
    }
}
