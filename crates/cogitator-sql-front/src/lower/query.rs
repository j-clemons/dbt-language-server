//! Queries: the WITH/body/ORDER BY/LIMIT shell around a body.

use cogitator_ir::common::HoleKind;
use cogitator_ir::hir::{
    Body, ExprId, ExprKind, OrderItem, OutputCol, Query, QueryId, ScopeId, ScopeKind,
};
use cogitator_ir::{DegradationKind, Span};
use sqlparser::ast::{self, SetExpr};

use super::binder::{AliasMode, Frame};
use super::Lowerer;

/// What the body of a query produced.
pub(super) struct Lowered {
    pub body: Body,
    pub outputs: Vec<OutputCol>,
    pub open: bool,
    pub span: Span,
}

impl Lowerer<'_> {
    /// Lowers a full query (`WITH` + body + `ORDER BY` + `LIMIT`).
    pub(super) fn query(&mut self, q: &ast::Query, kind: ScopeKind, barrier: bool) -> QueryId {
        let parent = self.current_scope();
        let scope = self.alloc_scope(kind, parent, Span::default());
        self.frames.push(Frame::new(scope, barrier));
        let saved_mode = std::mem::replace(&mut self.mode, AliasMode::Off);
        let mark = self.visible.len();
        let ctes = self.with_clause(q.with.as_ref());
        self.check_pipes(q);
        // A parenthesised body with no clauses of its own is that query.
        if let SetExpr::Query(inner) = q.body.as_ref() {
            if q.order_by.is_none() && q.limit_clause.is_none() && q.fetch.is_none() {
                let id = self.query(inner, ScopeKind::Query, false);
                self.prepend_ctes(id, ctes);
                self.leave(mark, saved_mode);
                return id;
            }
        }
        let lowered = self.set_expr_body(q.body.as_ref());
        self.bind_output_names(&lowered);
        self.mode = AliasMode::OrderBy;
        let order_by = self.order_by(q.order_by.as_ref());
        self.mode = AliasMode::Off;
        let limit = self.limit_clause(q);
        let id = self.alloc_query(
            Query {
                span: lowered.span,
                scope,
                ctes,
                body: lowered.body,
                order_by,
                limit,
                outputs: lowered.outputs,
            },
            lowered.open,
        );
        self.finish_scope(scope, lowered.span);
        self.leave(mark, saved_mode);
        id
    }

    fn check_pipes(&mut self, q: &ast::Query) {
        if !q.pipe_operators.is_empty() {
            self.degrade(
                DegradationKind::UnsupportedSyntax,
                Span::default(),
                "the pipe operator is not modelled",
            );
        }
    }

    fn leave(&mut self, mark: usize, mode: AliasMode) {
        self.visible.truncate(mark);
        self.frames.pop();
        self.mode = mode;
    }

    pub(super) fn finish_scope(&mut self, scope: ScopeId, span: Span) {
        let items = self
            .frames
            .last()
            .map(|f| f.items.clone())
            .unwrap_or_default();
        if let Some(s) = self.hir.scopes.get_mut(scope) {
            s.span = span;
            s.from = items;
        }
    }

    /// A SELECT's aliases are already bound by item index. A set
    /// operation has no inputs, so ORDER BY reads its output names, and
    /// a name missing from incomplete outputs stays quiet.
    fn bind_output_names(&mut self, lowered: &Lowered) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        frame.open_outputs = lowered.open && frame.items.is_empty();
        if !matches!(lowered.body, Body::Select(_)) {
            frame.set_aliases(
                lowered
                    .outputs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, col)| col.name.map(|name| (name, i as u32)))
                    .collect(),
            );
        }
    }

    pub(super) fn set_expr_body(&mut self, body: &SetExpr) -> Lowered {
        match body {
            SetExpr::Select(select) => self.select(select),
            SetExpr::SetOperation {
                left,
                op,
                set_quantifier,
                right,
            } => self.set_operation(left, *op, *set_quantifier, right),
            SetExpr::Values(values) => self.values(values),
            SetExpr::Query(_) => {
                self.unsupported_body("a parenthesised query with its own ORDER BY or LIMIT")
            }
            SetExpr::Table(_) => self.unsupported_body("a TABLE statement"),
            SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => {
                self.unsupported_body("a DML statement")
            }
        }
    }

    fn unsupported_body(&mut self, what: &str) -> Lowered {
        self.degrade(
            DegradationKind::UnsupportedSyntax,
            Span::default(),
            format!("{what} is not modelled"),
        );
        Lowered {
            body: Body::Hole(HoleKind::Unsupported),
            outputs: Vec::new(),
            open: true,
            span: Span::default(),
        }
    }

    pub(super) fn query_is_open(&self, q: QueryId) -> bool {
        self.query_open.get(q.index()).copied().unwrap_or(true)
    }

    fn values(&mut self, values: &ast::Values) -> Lowered {
        let rows: Vec<Vec<ExprId>> = values
            .rows
            .iter()
            .map(|row| row.iter().map(|e| self.expr(e)).collect())
            .collect();
        let arity = rows.first().map_or(0, Vec::len);
        let outputs = (0..arity)
            .map(|_| Self::output_col(Span::default(), None, None))
            .collect();
        let all: Vec<ExprId> = rows.iter().flatten().copied().collect();
        let span = self.union_spans(&all);
        Lowered {
            body: Body::Values(rows),
            outputs,
            open: false,
            span,
        }
    }

    fn order_by(&mut self, order: Option<&ast::OrderBy>) -> Vec<OrderItem> {
        match order.map(|o| &o.kind) {
            None => Vec::new(),
            Some(ast::OrderByKind::All(options)) => {
                let expr = self.alloc_expr(
                    Span::default(),
                    ExprKind::Opaque {
                        children: Vec::new(),
                    },
                );
                vec![OrderItem {
                    expr,
                    desc: sort_desc(options),
                    nulls_first: options.nulls_first,
                }]
            }
            Some(ast::OrderByKind::Expressions(items)) => self.order_items(items),
        }
    }

    pub(super) fn order_items(&mut self, items: &[ast::OrderByExpr]) -> Vec<OrderItem> {
        items
            .iter()
            .map(|item| OrderItem {
                expr: self.expr(&item.expr),
                desc: sort_desc(&item.options),
                nulls_first: item.options.nulls_first,
            })
            .collect()
    }

    fn limit_clause(&mut self, q: &ast::Query) -> Option<ExprId> {
        match &q.limit_clause {
            Some(ast::LimitClause::LimitOffset { limit, .. }) => {
                limit.as_ref().map(|e| self.expr(e))
            }
            Some(ast::LimitClause::OffsetCommaLimit { limit, .. }) => Some(self.expr(limit)),
            None => q
                .fetch
                .as_ref()
                .and_then(|f| f.quantity.as_ref())
                .map(|e| self.expr(e)),
        }
    }
}

fn sort_desc(options: &ast::OrderByOptions) -> Option<bool> {
    match options.sort {
        Some(ast::OrderBySort::Asc) => Some(false),
        Some(ast::OrderBySort::Desc) => Some(true),
        Some(ast::OrderBySort::Using(_)) | None => None,
    }
}
