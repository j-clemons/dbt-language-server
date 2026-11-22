//! Every id stored in an HIR node addresses a slot of its arena.

use crate::hir::{
    Body, ExprKind, From, FromKind, Hir, JoinCond, RelTarget, Resolution, SelectItem,
};
use crate::{Arena, Idx, Invariant, Violation};

struct Ids<'a> {
    out: &'a mut Vec<Violation>,
}

impl Ids<'_> {
    fn id<T>(&mut self, arena: &Arena<T>, id: Idx<T>, what: &str) {
        if !arena.contains(id) {
            self.out.push(Violation::new(
                Invariant::HirIdRange,
                format!("{what} {id:?} is out of range ({})", arena.len()),
            ));
        }
    }

    fn all<T>(&mut self, arena: &Arena<T>, ids: impl IntoIterator<Item = Idx<T>>, what: &str) {
        for id in ids {
            self.id(arena, id, what);
        }
    }
}

pub(super) fn check(hir: &Hir, out: &mut Vec<Violation>) {
    let mut c = Ids { out };
    if let Some(root) = hir.root {
        c.id(&hir.queries, root, "root");
    }
    for (_, scope) in hir.scopes.iter() {
        if let Some(parent) = scope.parent {
            c.id(&hir.scopes, parent, "scope parent");
        }
        c.all(&hir.from, scope.from.iter().copied(), "scope from");
    }
    for (_, cte) in hir.ctes.iter() {
        c.id(&hir.queries, cte.query, "cte query");
    }
    for (_, query) in hir.queries.iter() {
        c.id(&hir.scopes, query.scope, "query scope");
        c.all(&hir.ctes, query.ctes.iter().copied(), "query cte");
        c.all(&hir.exprs, query.exprs(), "query expr");
        match &query.body {
            Body::SetOp { l, r, .. } => {
                c.id(&hir.queries, *l, "setop left");
                c.id(&hir.queries, *r, "setop right");
            }
            Body::Select(select) => {
                c.all(&hir.from, select.from.iter().copied(), "select from");
                for item in &select.items {
                    match item {
                        SelectItem::Jinja(j) => c.id(&hir.jinja, *j, "select jinja"),
                        SelectItem::Star { from: Some(f), .. } => {
                            c.id(&hir.from, *f, "star from");
                        }
                        _ => {}
                    }
                }
            }
            Body::Values(_) | Body::Hole(_) => {}
        }
        for col in &query.outputs {
            c.all(&hir.exprs, col.expr, "output expr");
        }
    }
    for (_, item) in hir.from.iter() {
        check_from(&mut c, hir, item);
    }
    for (_, expr) in hir.exprs.iter() {
        check_expr(&mut c, hir, &expr.kind);
    }
    for (_, col) in hir.cols.iter() {
        if let Some(res) = &col.res {
            check_resolution(&mut c, hir, res);
        }
    }
}

fn check_from(c: &mut Ids<'_>, hir: &Hir, item: &From) {
    match &item.kind {
        FromKind::Base { target, .. } => match target {
            RelTarget::Cte(cte) => c.id(&hir.ctes, *cte, "from cte"),
            RelTarget::Jinja(j) => c.id(&hir.jinja, *j, "from jinja"),
            RelTarget::Model { .. }
            | RelTarget::Source { .. }
            | RelTarget::This
            | RelTarget::Physical(_) => {}
        },
        FromKind::Derived { q, .. } => c.id(&hir.queries, *q, "derived query"),
        FromKind::Join { l, r, on, .. } => {
            c.id(&hir.from, *l, "join left");
            c.id(&hir.from, *r, "join right");
            if let JoinCond::On(expr) = on {
                c.id(&hir.exprs, *expr, "join on");
            }
        }
        FromKind::Unnest { exprs, .. } => c.all(&hir.exprs, exprs.iter().copied(), "unnest expr"),
        FromKind::TableFn { args, .. } => c.all(&hir.exprs, args.iter().copied(), "tablefn arg"),
        FromKind::Hole => {}
    }
}

fn check_expr(c: &mut Ids<'_>, hir: &Hir, kind: &ExprKind) {
    c.all(&hir.exprs, kind.children(), "expr child");
    if let Some(q) = kind.subquery() {
        c.id(&hir.queries, q, "subquery");
    }
    match kind {
        ExprKind::Col(col) => c.id(&hir.cols, *col, "col ref"),
        ExprKind::Jinja(j) => c.id(&hir.jinja, *j, "expr jinja"),
        _ => {}
    }
}

fn check_resolution(c: &mut Ids<'_>, hir: &Hir, res: &Resolution) {
    match res {
        Resolution::Bound { from, col } | Resolution::Outer { from, col, .. } => {
            c.id(&hir.from, *from, "resolution from");
            if let Some(item) = hir.from.get(*from) {
                if !item.open && *col as usize >= item.output.len() {
                    c.out.push(Violation::new(
                        Invariant::HirIdRange,
                        format!("resolution col {col} exceeds from {from:?} output"),
                    ));
                }
            }
        }
        Resolution::OpenSchema(from) => c.id(&hir.from, *from, "resolution from"),
        Resolution::Candidates(items) => {
            c.all(&hir.from, items.iter().copied(), "candidate from");
        }
        Resolution::LateralAlias(_) | Resolution::Unresolved => {}
    }
}
