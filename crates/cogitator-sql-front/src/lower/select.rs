//! SELECT: FROM first (it opens the scope), then the select list with
//! its lateral aliases, then the remaining clauses.

use cogitator_ir::hir::{
    Body, Distinct, ExprId, ExprKind, GroupBy, Ident, JinjaRole, OutputCol, Resolution, Select,
    SelectItem, WindowSpec,
};
use cogitator_ir::types::Nullability;
use cogitator_ir::{ColType, DegradationKind, Span, Symbol};
use sqlparser::ast;

use super::binder::AliasMode;
use super::query::Lowered;
use super::{union, Lowerer};
use crate::mask::RegionKind;

/// One select item lowered, with what it adds to the query's outputs.
pub(super) struct ItemOut {
    pub item: SelectItem,
    pub outputs: Vec<OutputCol>,
    pub open: bool,
    /// The alias visible to later items, if the item has a single name.
    pub alias: Option<Symbol>,
}

impl Lowerer<'_> {
    pub(super) fn select(&mut self, s: &ast::Select) -> Lowered {
        self.note_unsupported_select(s);
        let from = self.lower_from(&s.from);
        let (items, outputs, open) = self.projection(&s.projection);
        let distinct = self.distinct(s.distinct.as_ref());
        self.mode = AliasMode::Lateral;
        let filter = s.selection.as_ref().map(|e| self.expr(e));
        self.mode = AliasMode::GroupBy;
        let group_by = self.group_by(&s.group_by);
        self.mode = AliasMode::Lateral;
        let having = s.having.as_ref().map(|e| self.expr(e));
        let qualify = s.qualify.as_ref().map(|e| self.expr(e));
        let windows = self.named_windows(&s.named_window);
        self.mode = AliasMode::Off;
        let span = self.select_span(s, &items);
        let select = Select {
            span,
            distinct,
            items,
            from,
            filter,
            group_by,
            having,
            qualify,
            windows,
        };
        Lowered {
            body: Body::Select(Box::new(select)),
            outputs,
            open,
            span,
        }
    }

    fn distinct(&mut self, distinct: Option<&ast::Distinct>) -> Distinct {
        match distinct {
            None | Some(ast::Distinct::All) => Distinct::None,
            Some(ast::Distinct::Distinct) => Distinct::All,
            Some(ast::Distinct::On(exprs)) => {
                Distinct::On(exprs.iter().map(|e| self.expr(e)).collect())
            }
        }
    }

    /// Exact start (the SELECT keyword), end derived from the items.
    fn select_span(&self, s: &ast::Select, items: &[SelectItem]) -> Span {
        let start = self.sql_span(s.select_token.0.span).unwrap_or_default();
        items
            .iter()
            .filter_map(|item| self.item_span(item))
            .fold(start, union)
    }

    fn item_span(&self, item: &SelectItem) -> Option<Span> {
        match item {
            SelectItem::Expr { expr, .. } => self.hir.exprs.get(*expr).map(|e| e.span),
            SelectItem::Star { span, .. } | SelectItem::Hole(span) => Some(*span),
            SelectItem::Jinja(id) => self.hir.jinja.get(*id).map(|j| j.span),
        }
    }

    fn note_unsupported_select(&mut self, s: &ast::Select) {
        if s.exclude.is_some() || !s.lateral_views.is_empty() || !s.connect_by.is_empty() {
            self.degrade(
                DegradationKind::UnsupportedSyntax,
                Span::default(),
                "EXCLUDE on SELECT, LATERAL VIEW or CONNECT BY is not modelled",
            );
        }
    }

    fn projection(
        &mut self,
        projection: &[ast::SelectItem],
    ) -> (Vec<SelectItem>, Vec<OutputCol>, bool) {
        let (mut items, mut outputs, mut open) = (Vec::new(), Vec::new(), false);
        for (index, item) in projection.iter().enumerate() {
            self.mode = AliasMode::Lateral;
            let out = self.select_item(item);
            open |= out.open;
            if let (Some(name), Some(frame)) = (out.alias, self.frames.last_mut()) {
                frame.aliases.push((name, index as u32));
            }
            outputs.extend(out.outputs);
            items.push(out.item);
        }
        self.mode = AliasMode::Off;
        (items, outputs, open)
    }

    fn select_item(&mut self, item: &ast::SelectItem) -> ItemOut {
        match item {
            ast::SelectItem::UnnamedExpr(e) => self.unnamed_item(e),
            ast::SelectItem::ExprWithAlias { expr, alias } => self.aliased_item(expr, alias),
            ast::SelectItem::ExprWithAliases { expr, .. } => {
                self.degrade(
                    DegradationKind::UnsupportedSyntax,
                    Span::default(),
                    "a multi-alias select item is not modelled",
                );
                let e = self.expr(expr);
                ItemOut {
                    item: SelectItem::Expr {
                        expr: e,
                        alias: None,
                    },
                    outputs: Vec::new(),
                    open: true,
                    alias: None,
                }
            }
            ast::SelectItem::QualifiedWildcard(kind, options) => self.qualified_star(kind, options),
            ast::SelectItem::Wildcard(options) => self.star(None, options),
        }
    }

    fn aliased_item(&mut self, expr: &ast::Expr, alias: &ast::Ident) -> ItemOut {
        let e = self.expr(expr);
        let alias = self.alias_ident(alias);
        let span = self.expr_span(e);
        let name = alias.map(|a| a.folded);
        let mut col = Self::output_col(span, name, Some(e));
        col.ty = self.type_of(e);
        ItemOut {
            item: SelectItem::Expr { expr: e, alias },
            outputs: vec![col],
            open: false,
            alias: name,
        }
    }

    fn unnamed_item(&mut self, e: &ast::Expr) -> ItemOut {
        if let Some(region) = self.sole_placeholder(e) {
            if let Some(item) = self.placeholder_item(region) {
                return ItemOut {
                    item,
                    outputs: Vec::new(),
                    open: true,
                    alias: None,
                };
            }
        }
        let id = self.expr(e);
        let name = self.output_name(e);
        let mut col = Self::output_col(self.expr_span(id), name, Some(id));
        col.ty = self.type_of(id);
        ItemOut {
            item: SelectItem::Expr {
                expr: id,
                alias: None,
            },
            outputs: vec![col],
            open: false,
            alias: None,
        }
    }

    /// The region when `e` is nothing but a placeholder identifier.
    fn sole_placeholder(&self, e: &ast::Expr) -> Option<usize> {
        match e {
            ast::Expr::Identifier(ident) => self.region_at(self.ident_span(ident)),
            _ => None,
        }
    }

    /// A macro alone in the select list may expand to many columns.
    fn placeholder_item(&mut self, region: usize) -> Option<SelectItem> {
        let role = match self.region(region).map(|r| &r.kind) {
            Some(RegionKind::Macro(_)) => JinjaRole::Composite,
            _ => JinjaRole::Scalar,
        };
        self.take_region(region, role).map(SelectItem::Jinja)
    }

    /// Plain `col` or `t.col` names its output; nothing else does.
    fn output_name(&mut self, e: &ast::Expr) -> Option<Symbol> {
        let ident = match e {
            ast::Expr::Identifier(i) => i,
            ast::Expr::CompoundIdentifier(path) => path.last()?,
            _ => return None,
        };
        if self.region_at(self.ident_span(ident)).is_some() {
            return None;
        }
        Some(self.sym_for(ident))
    }

    fn expr_span(&self, id: ExprId) -> Span {
        self.hir.exprs.get(id).map_or(Span::default(), |e| e.span)
    }

    /// The type a select item carries forward: column types pass through.
    fn type_of(&self, id: ExprId) -> ColType {
        let Some(expr) = self.hir.exprs.get(id) else {
            return ColType::UNKNOWN;
        };
        match &expr.kind {
            ExprKind::Col(col) => {
                let res = self.hir.cols.get(*col).and_then(|c| c.res.as_ref());
                let bound = match res {
                    Some(Resolution::Bound { from, col }) => Some((*from, *col)),
                    _ => None,
                };
                bound
                    .and_then(|(from, col)| self.hir.from.get(from)?.output.get(col as usize))
                    .map_or(ColType::UNKNOWN, |c| c.ty.clone()) // boundary: per-plan types
            }
            ExprKind::Cast { ty, .. } => ColType::new(ty.clone(), Nullability::Unknown),
            _ => ColType::UNKNOWN,
        }
    }

    fn group_by(&mut self, group: &ast::GroupByExpr) -> GroupBy {
        let exprs = match group {
            ast::GroupByExpr::All(_) => return GroupBy::All,
            ast::GroupByExpr::Expressions(exprs, _) => exprs,
        };
        match exprs.as_slice() {
            [] => GroupBy::None,
            [ast::Expr::Rollup(sets)] => GroupBy::Rollup(self.expr_sets_flat(sets)),
            [ast::Expr::Cube(sets)] => GroupBy::Cube(self.expr_sets_flat(sets)),
            [ast::Expr::GroupingSets(sets)] => GroupBy::Sets(
                sets.iter()
                    .map(|set| set.iter().map(|e| self.expr(e)).collect())
                    .collect(),
            ),
            _ => GroupBy::Exprs(exprs.iter().map(|e| self.expr(e)).collect()),
        }
    }

    fn expr_sets_flat(&mut self, sets: &[Vec<ast::Expr>]) -> Vec<ExprId> {
        sets.iter().flatten().map(|e| self.expr(e)).collect()
    }

    fn named_windows(
        &mut self,
        windows: &[ast::NamedWindowDefinition],
    ) -> Vec<(Ident, WindowSpec)> {
        windows
            .iter()
            .map(|ast::NamedWindowDefinition(name, def)| {
                let name = self.ident(name);
                let spec = match def {
                    ast::NamedWindowExpr::NamedWindow(other) => WindowSpec {
                        name: Some(self.ident(other)),
                        ..WindowSpec::default()
                    },
                    ast::NamedWindowExpr::WindowSpec(spec) => self.window_spec(spec),
                };
                (name, spec)
            })
            .collect()
    }
}
