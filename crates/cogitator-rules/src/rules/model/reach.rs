//! The CTE reference graph of a HIR, walked from the root query.

use std::collections::BTreeSet;

use cogitator_ir::hir::{
    Body, CteId, ExprId, FromId, FromKind, Hir, JoinCond, Query, QueryId, RelTarget,
};

/// Every CTE any FROM item names, wherever it sits.
pub(super) fn referenced_anywhere(hir: &Hir) -> BTreeSet<CteId> {
    hir.from
        .iter()
        .filter_map(|(_, f)| match &f.kind {
            FromKind::Base {
                target: RelTarget::Cte(c),
                ..
            } => Some(*c),
            _ => None,
        })
        .collect()
}

/// CTEs reachable from `root`, through CTE bodies, derived tables and
/// subqueries. Ids index the arenas, so `seen` bounds the walk.
pub(super) fn reachable_ctes(hir: &Hir, root: QueryId) -> BTreeSet<CteId> {
    let mut used = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut queries = vec![root];
    while let Some(id) = queries.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(query) = hir.queries.get(id) else {
            continue;
        };
        let (from, exprs) = query_parts(query, &mut queries);
        walk_from(hir, from, exprs, &mut queries, &mut used);
    }
    used
}

/// A query's own FROM items and expressions, with set-operation arms
/// queued for `reachable_ctes`.
fn query_parts(query: &Query, queries: &mut Vec<QueryId>) -> (Vec<FromId>, Vec<ExprId>) {
    let from = match &query.body {
        Body::Select(select) => select.from.clone(),
        Body::SetOp { l, r, .. } => {
            queries.push(*l);
            queries.push(*r);
            Vec::new()
        }
        Body::Values(_) | Body::Hole(_) => Vec::new(),
    };
    (from, query.exprs())
}

fn walk_from(
    hir: &Hir,
    mut from: Vec<FromId>,
    mut exprs: Vec<ExprId>,
    queries: &mut Vec<QueryId>,
    used: &mut BTreeSet<CteId>,
) {
    let mut seen_from = BTreeSet::new();
    let mut seen_exprs = BTreeSet::new();
    loop {
        if let Some(id) = from.pop() {
            if !seen_from.insert(id) {
                continue;
            }
            let Some(item) = hir.from.get(id) else {
                continue;
            };
            match &item.kind {
                FromKind::Base {
                    target: RelTarget::Cte(cte),
                    ..
                } => {
                    if used.insert(*cte) {
                        if let Some(def) = hir.ctes.get(*cte) {
                            queries.push(def.query);
                        }
                    }
                }
                FromKind::Derived { q, .. } => queries.push(*q),
                FromKind::Join { l, r, on, .. } => {
                    from.push(*l);
                    from.push(*r);
                    if let JoinCond::On(e) = on {
                        exprs.push(*e);
                    }
                }
                FromKind::Unnest { exprs: list, .. } | FromKind::TableFn { args: list, .. } => {
                    exprs.extend(list.iter().copied());
                }
                FromKind::Base { .. } | FromKind::Hole => {}
            }
        } else if let Some(id) = exprs.pop() {
            if !seen_exprs.insert(id) {
                continue;
            }
            if let Some(node) = hir.exprs.get(id) {
                if let Some(q) = node.kind.subquery() {
                    queries.push(q);
                }
                exprs.extend(node.kind.children());
            }
        } else {
            return;
        }
    }
}
