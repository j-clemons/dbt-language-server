//! Expression sources: every column reference an expression reads.

use cogitator_ir::hir::{ColRef, ColRefId, ExprId, ExprKind, Resolution, SelectItem, WindowSpec};

use super::lookup::Wanted;
use super::{unique, Ctx, Walker};
use crate::lineage::SourceRef;

impl Walker<'_> {
    /// Every source `expr` reads, deduplicated, first reference first.
    pub(super) fn expr_sources(&mut self, expr: ExprId, ctx: Ctx<'_>) -> Vec<SourceRef> {
        let mut out = Vec::new();
        self.walk_expr(expr, ctx, &mut out);
        unique(out)
    }

    pub(super) fn exprs_sources(&mut self, exprs: &[ExprId], ctx: Ctx<'_>) -> Vec<SourceRef> {
        let mut out = Vec::new();
        for expr in exprs {
            self.walk_expr(*expr, ctx, &mut out);
        }
        unique(out)
    }

    pub(super) fn walk_expr(&mut self, id: ExprId, ctx: Ctx<'_>, out: &mut Vec<SourceRef>) {
        let hir = self.hir;
        let Some(expr) = hir.exprs.get(id) else {
            return;
        };
        match &expr.kind {
            ExprKind::Col(col) => self.walk_col(*col, ctx, out),
            ExprKind::Jinja(_) | ExprKind::Lit(_) | ExprKind::Hole => {}
            ExprKind::Exists(q) => out.extend(self.subquery_sources(*q, true)),
            ExprKind::Scalar(q) => out.extend(self.subquery_sources(*q, false)),
            kind => {
                if let ExprKind::InQuery { q, .. } = kind {
                    out.extend(self.subquery_sources(*q, false));
                }
                if let ExprKind::Call {
                    over: Some(spec), ..
                } = kind
                {
                    self.walk_named_window(spec, ctx, out);
                }
                for child in kind.children() {
                    self.walk_expr(child, ctx, out);
                }
            }
        }
    }

    /// `over w` reads what `w` partitions and orders by, following chains
    /// of named windows at most once per definition.
    fn walk_named_window(&mut self, spec: &WindowSpec, ctx: Ctx<'_>, out: &mut Vec<SourceRef>) {
        let Some(select) = ctx.select else {
            return;
        };
        let mut name = spec.name;
        for _ in 0..select.windows.len() {
            let Some(wanted) = name else {
                break;
            };
            let Some((_, definition)) = select
                .windows
                .iter()
                .find(|(n, _)| n.folded == wanted.folded)
            else {
                break;
            };
            for expr in definition.exprs() {
                self.walk_expr(expr, ctx, out);
            }
            name = definition.name;
        }
    }

    fn walk_col(&mut self, id: ColRefId, ctx: Ctx<'_>, out: &mut Vec<SourceRef>) {
        let hir = self.hir;
        let Some(col) = hir.cols.get(id) else {
            return;
        };
        let label = self.surface(col.name);
        let wanted = Wanted {
            key: Some(col.name.folded),
            label: &label,
            bound: None,
        };
        match &col.res {
            Some(Resolution::Bound { from, col }) | Some(Resolution::Outer { from, col, .. }) => {
                let wanted = Wanted {
                    bound: Some(*col),
                    ..wanted
                };
                out.extend(self.item_sources(*from, wanted, ctx));
            }
            Some(Resolution::OpenSchema(from)) => {
                out.extend(self.item_sources(*from, wanted, ctx));
            }
            // Empty candidates: ORDER BY over an open set operation
            Some(Resolution::Candidates(from)) => {
                for candidate in from {
                    out.extend(self.item_sources(*candidate, wanted, ctx));
                }
            }
            Some(Resolution::LateralAlias(item)) => self.walk_alias(*item, ctx, out),
            Some(Resolution::Unresolved) => out.extend(self.unresolved(col, wanted, ctx)),
            None => {}
        }
    }

    fn walk_alias(&mut self, item: u32, ctx: Ctx<'_>, out: &mut Vec<SourceRef>) {
        let Some(select) = ctx.select else {
            return;
        };
        if let Some(SelectItem::Expr { expr, .. }) = select.items.get(item as usize) {
            self.walk_expr(*expr, ctx, out);
        }
    }

    /// A name the binder could not place, which over closed CTEs means the
    /// column is missing: report it against the relation it was asked of.
    fn unresolved(&mut self, col: &ColRef, wanted: Wanted<'_>, ctx: Ctx<'_>) -> Vec<SourceRef> {
        if let Some(qualifier) = col.qualifier.last() {
            return match self.leaf_named(*qualifier, ctx) {
                Some(leaf) => self.item_sources(leaf, wanted, ctx),
                None => vec![self.unknown(self.surface(*qualifier), wanted)],
            };
        }
        let mut out = Vec::new();
        for leaf in self.scope_leaves(ctx) {
            out.extend(self.item_sources(leaf, wanted, ctx));
        }
        out
    }
}
