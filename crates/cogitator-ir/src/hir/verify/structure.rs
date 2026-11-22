//! Structural HIR invariants; ids are in range when these run.

use crate::hir::{
    Body, CteId, ExprId, ExprKind, FromKind, Hir, JinjaId, JinjaRole, QueryId, RelTarget,
    SelectItem,
};
use crate::{Invariant, Span, Violation};

/// Bounds the query-nesting recursion below. Well above the front
/// end's expression-depth cap; deeper input is reported, not followed.
const MAX_QUERY_DEPTH: usize = 512;

pub(super) fn scope_tree(hir: &Hir, out: &mut Vec<Violation>) {
    let bad = |detail: String| Violation::new(Invariant::HirScopeTree, detail);
    let roots = hir.scopes.values().filter(|s| s.parent.is_none()).count();
    if roots != 1 && !hir.scopes.is_empty() {
        out.push(bad(format!("expected one root scope, found {roots}")));
    }
    for (id, _) in hir.scopes.iter() {
        let mut steps = 0;
        let mut at = id;
        while let Some(parent) = hir.scopes.get(at).and_then(|s| s.parent) {
            steps += 1;
            if steps > hir.scopes.len() {
                out.push(bad(format!("scope {id:?} is on a parent cycle")));
                break;
            }
            at = parent;
        }
    }
}

pub(super) fn col_refs_resolved(hir: &Hir, out: &mut Vec<Violation>) {
    for (id, col) in hir.cols.iter() {
        if col.res.is_none() {
            out.push(Violation::new(
                Invariant::HirColRefResolved,
                format!("col ref {id:?} was never bound"),
            ));
        }
    }
}

/// CTE references must name a CTE declared earlier in the same or an
/// enclosing WITH, or the CTE itself when it is recursive.
pub(super) fn cte_backward(hir: &Hir, out: &mut Vec<Violation>) {
    let mut seen = vec![false; hir.queries.len()];
    let Some(root) = hir.root else { return };
    let mut visible = Vec::new();
    walk_query(hir, root, &mut visible, &mut seen, 0, out);
}

fn walk_query(
    hir: &Hir,
    id: QueryId,
    visible: &mut Vec<CteId>,
    seen: &mut [bool],
    depth: usize,
    out: &mut Vec<Violation>,
) {
    if depth > MAX_QUERY_DEPTH {
        out.push(Violation::new(
            Invariant::HirCteBackward,
            format!("query nesting exceeds {MAX_QUERY_DEPTH} at {id:?}"),
        ));
        return;
    }
    let Some(query) = hir.queries.get(id) else {
        return;
    };
    if std::mem::replace(&mut seen[id.index()], true) {
        return;
    }
    let mark = visible.len();
    for &cte_id in &query.ctes {
        let Some(cte) = hir.ctes.get(cte_id) else {
            continue;
        };
        if cte.recursive {
            visible.push(cte_id);
        }
        walk_query(hir, cte.query, visible, seen, depth + 1, out);
        if !cte.recursive {
            visible.push(cte_id);
        }
    }
    match &query.body {
        Body::SetOp { l, r, .. } => {
            walk_query(hir, *l, visible, seen, depth + 1, out);
            walk_query(hir, *r, visible, seen, depth + 1, out);
        }
        Body::Select(select) => {
            for &f in &select.from {
                walk_from(hir, f, visible, seen, depth, out);
            }
        }
        Body::Values(_) | Body::Hole(_) => {}
    }
    for expr in query.exprs() {
        walk_expr(hir, expr, visible, seen, depth, out);
    }
    visible.truncate(mark);
}

fn walk_from(
    hir: &Hir,
    root: crate::hir::FromId,
    visible: &mut Vec<CteId>,
    seen: &mut [bool],
    depth: usize,
    out: &mut Vec<Violation>,
) {
    let mut stack = vec![root];
    let mut guard = 0;
    while let Some(id) = stack.pop() {
        guard += 1;
        if guard > hir.from.len() {
            break;
        }
        let Some(item) = hir.from.get(id) else {
            continue;
        };
        match &item.kind {
            FromKind::Base {
                target: RelTarget::Cte(cte),
                ..
            } if !visible.contains(cte) => {
                out.push(Violation::new(
                    Invariant::HirCteBackward,
                    format!("from {id:?} references cte {cte:?} before its declaration"),
                ));
            }
            FromKind::Derived { q, .. } => walk_query(hir, *q, visible, seen, depth + 1, out),
            FromKind::Join { l, r, on, .. } => {
                stack.push(*r);
                stack.push(*l);
                if let crate::hir::JoinCond::On(expr) = on {
                    walk_expr(hir, *expr, visible, seen, depth, out);
                }
            }
            FromKind::Unnest { exprs, .. } | FromKind::TableFn { args: exprs, .. } => {
                for expr in exprs {
                    walk_expr(hir, *expr, visible, seen, depth, out);
                }
            }
            FromKind::Base { .. } | FromKind::Hole => {}
        }
    }
}

fn walk_expr(
    hir: &Hir,
    root: ExprId,
    visible: &mut Vec<CteId>,
    seen: &mut [bool],
    depth: usize,
    out: &mut Vec<Violation>,
) {
    let mut stack = vec![root];
    let mut guard = 0;
    while let Some(id) = stack.pop() {
        guard += 1;
        if guard > hir.exprs.len() {
            break;
        }
        let Some(expr) = hir.exprs.get(id) else {
            continue;
        };
        if let Some(q) = expr.kind.subquery() {
            walk_query(hir, q, visible, seen, depth + 1, out);
        }
        stack.extend(expr.kind.children());
    }
}

/// Each Relation/Scalar/Composite region is consumed exactly once;
/// Blank and ClausePosition regions may stand alone but not be shared.
pub(super) fn jinja_consumed(hir: &Hir, out: &mut Vec<Violation>) {
    let mut uses = vec![0u32; hir.jinja.len()];
    let mut mark = |id: JinjaId| {
        if let Some(n) = uses.get_mut(id.index()) {
            *n += 1;
        }
    };
    for expr in hir.exprs.values() {
        if let ExprKind::Jinja(id) = expr.kind {
            mark(id);
        }
    }
    for item in hir.from.values() {
        if let FromKind::Base {
            target: RelTarget::Jinja(id),
            ..
        } = item.kind
        {
            mark(id);
        }
    }
    for query in hir.queries.values() {
        if let Body::Select(select) = &query.body {
            for item in &select.items {
                if let SelectItem::Jinja(id) = item {
                    mark(*id);
                }
            }
        }
    }
    for (id, node) in hir.jinja.iter() {
        let count = uses[id.index()];
        let standalone = matches!(node.role, JinjaRole::Blank | JinjaRole::ClausePosition);
        if count > 1 || (count == 0 && !standalone) {
            out.push(Violation::new(
                Invariant::HirJinjaConsumed,
                format!("jinja {id:?} ({:?}) consumed {count} times", node.role),
            ));
        }
    }
}

pub(super) fn spans(hir: &Hir, text: &str, out: &mut Vec<Violation>) {
    let mut check = |span: Span, what: &str| {
        if !span.is_valid_in(text) {
            out.push(Violation::new(
                Invariant::HirSpanBoundary,
                format!("{what} span {}..{} is invalid", span.start, span.end),
            ));
        }
    };
    hir.queries.values().for_each(|q| check(q.span, "query"));
    hir.scopes.values().for_each(|s| check(s.span, "scope"));
    hir.ctes.values().for_each(|c| {
        check(c.span, "cte");
        check(c.name.span, "cte name");
    });
    hir.from.values().for_each(|f| check(f.span, "from"));
    hir.exprs.values().for_each(|e| check(e.span, "expr"));
    hir.cols.values().for_each(|c| {
        check(c.span, "col");
        check(c.name.span, "col name");
    });
    hir.jinja.values().for_each(|j| check(j.span, "jinja"));
}
