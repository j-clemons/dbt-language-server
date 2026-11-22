//! Structural equality of HIR expressions, ignoring spans. Matches
//! select items against group keys and ORDER BY keys.

use cogitator_ir::hir::{ColRefId, Expr, ExprId, ExprKind, Hir, Resolution};

/// Bounds the walk on unverified HIR, which could contain a cycle.
const MAX_DEPTH: usize = 64;

pub(super) fn same_expr(hir: &Hir, a: ExprId, b: ExprId) -> bool {
    eq(hir, a, b, 0)
}

fn eq(hir: &Hir, a: ExprId, b: ExprId, depth: usize) -> bool {
    if a == b {
        return true;
    }
    if depth > MAX_DEPTH {
        return false;
    }
    let (Some(x), Some(y)): (Option<&Expr>, Option<&Expr>) = (hir.exprs.get(a), hir.exprs.get(b))
    else {
        return false;
    };
    let all = |xs: &[ExprId], ys: &[ExprId]| {
        xs.len() == ys.len() && xs.iter().zip(ys).all(|(p, q)| eq(hir, *p, *q, depth + 1))
    };
    let opt = |p: Option<ExprId>, q: Option<ExprId>| match (p, q) {
        (Some(p), Some(q)) => eq(hir, p, q, depth + 1),
        (None, None) => true,
        _ => false,
    };
    match (&x.kind, &y.kind) {
        (ExprKind::Col(p), ExprKind::Col(q)) => same_col(hir, *p, *q),
        (ExprKind::Lit(p), ExprKind::Lit(q)) => p == q,
        (ExprKind::Unary { op: o1, e: e1 }, ExprKind::Unary { op: o2, e: e2 }) => {
            o1 == o2 && eq(hir, *e1, *e2, depth + 1)
        }
        (
            ExprKind::Binary {
                op: o1,
                l: l1,
                r: r1,
            },
            ExprKind::Binary {
                op: o2,
                l: l2,
                r: r2,
            },
        ) => o1 == o2 && eq(hir, *l1, *l2, depth + 1) && eq(hir, *r1, *r2, depth + 1),
        (
            ExprKind::Call {
                name: n1,
                args: a1,
                distinct: d1,
                filter: f1,
                over: None,
            },
            ExprKind::Call {
                name: n2,
                args: a2,
                distinct: d2,
                filter: f2,
                over: None,
            },
        ) => n1 == n2 && d1 == d2 && all(a1, a2) && opt(*f1, *f2),
        (ExprKind::Cast { e: e1, ty: t1 }, ExprKind::Cast { e: e2, ty: t2 }) => {
            t1 == t2 && eq(hir, *e1, *e2, depth + 1)
        }
        (
            ExprKind::Case {
                operand: o1,
                arms: a1,
                else_: e1,
            },
            ExprKind::Case {
                operand: o2,
                arms: a2,
                else_: e2,
            },
        ) => {
            a1.len() == a2.len()
                && opt(*o1, *o2)
                && opt(*e1, *e2)
                && a1.iter().zip(a2).all(|((w1, t1), (w2, t2))| {
                    eq(hir, *w1, *w2, depth + 1) && eq(hir, *t1, *t2, depth + 1)
                })
        }
        (ExprKind::Jinja(p), ExprKind::Jinja(q)) => p == q,
        _ => false,
    }
}

fn same_col(hir: &Hir, a: ColRefId, b: ColRefId) -> bool {
    let (Some(x), Some(y)) = (hir.cols.get(a), hir.cols.get(b)) else {
        return false;
    };
    match (&x.res, &y.res) {
        (
            Some(Resolution::Bound { from: f1, col: c1 }),
            Some(Resolution::Bound { from: f2, col: c2 }),
        ) => f1 == f2 && c1 == c2,
        (Some(Resolution::OpenSchema(f1)), Some(Resolution::OpenSchema(f2))) => {
            f1 == f2 && x.name.folded == y.name.folded
        }
        _ => false,
    }
}
