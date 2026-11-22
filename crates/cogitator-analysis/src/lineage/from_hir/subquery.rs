//! Subquery expressions: unlike a CTE, nothing above them can pick a
//! column, so every clause of the body decides the value.

use cogitator_ir::hir::{Body, FromId, FromKind, JoinCond, QueryId, SelectItem};

use super::{unique, Ctx, Walker};
use crate::lineage::SourceRef;

impl Walker<'_> {
    /// All sources a subquery reads. EXISTS ignores its select list's stars,
    /// which only say "any row": `skip_stars`.
    pub(super) fn subquery_sources(&mut self, id: QueryId, skip_stars: bool) -> Vec<SourceRef> {
        let mut out = Vec::new();
        self.walk_subquery(id, skip_stars, &mut out);
        unique(out)
    }

    fn walk_subquery(&mut self, id: QueryId, skip_stars: bool, out: &mut Vec<SourceRef>) {
        let hir = self.hir;
        let Some(query) = hir.queries.get(id) else {
            return;
        };
        if let Body::SetOp { l, r, .. } = &query.body {
            self.walk_subquery(*l, skip_stars, out);
            self.walk_subquery(*r, skip_stars, out);
        }
        let select = match &query.body {
            Body::Select(select) => Some(select.as_ref()),
            _ => None,
        };
        let ctx = Ctx {
            scope: query.scope,
            select,
        };
        if let Some(select) = select {
            if !skip_stars {
                self.walk_star_items(&select.items, ctx, out);
            }
            for from in &select.from {
                self.walk_join_conditions(*from, ctx, out);
            }
        }
        for expr in query.exprs() {
            self.walk_expr(expr, ctx, out);
        }
    }

    fn walk_star_items(&mut self, items: &[SelectItem], ctx: Ctx<'_>, out: &mut Vec<SourceRef>) {
        for item in items {
            let SelectItem::Star {
                from, qualifier, ..
            } = item
            else {
                continue;
            };
            let targets = match (from, qualifier) {
                (Some(from), _) => vec![*from],
                (None, Some(_)) => Vec::new(),
                (None, None) => self.scope_leaves(ctx),
            };
            for target in targets {
                out.extend(self.star_of_item(target, ctx));
            }
        }
    }

    fn walk_join_conditions(&mut self, id: FromId, ctx: Ctx<'_>, out: &mut Vec<SourceRef>) {
        let hir = self.hir;
        let Some(item) = hir.from.get(id) else {
            return;
        };
        if let FromKind::Join { l, r, on, .. } = &item.kind {
            self.walk_join_conditions(*l, ctx, out);
            self.walk_join_conditions(*r, ctx, out);
            if let JoinCond::On(expr) = on {
                self.walk_expr(*expr, ctx, out);
            }
        }
    }
}
