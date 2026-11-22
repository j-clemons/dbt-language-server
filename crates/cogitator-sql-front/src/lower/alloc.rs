//! Arena allocation, the node budget and degradation reporting.

use cogitator_ir::common::HoleKind;
use cogitator_ir::hir::{
    node_count, Body, CteId, Expr, ExprId, ExprKind, From, FromCol, FromId, FromKind, OutputCol,
    Query, QueryId, Scope, ScopeId, ScopeKind,
};
use cogitator_ir::{Degradation, DegradationKind, Span, Symbol};

use super::{name_index, union, Lowerer, INDEXED_WIDTH};
use crate::caps::MAX_NODES;

impl Lowerer<'_> {
    pub(super) fn prepend_ctes(&mut self, id: QueryId, ctes: Vec<CteId>) {
        if let Some(query) = self.hir.queries.get_mut(id) {
            let own = std::mem::replace(&mut query.ctes, ctes);
            query.ctes.extend(own);
        }
    }

    pub(super) fn hole_query(&mut self, kind: HoleKind, span: Span, ctes: Vec<CteId>) -> QueryId {
        let parent = self.current_scope();
        let scope = self.alloc_scope(ScopeKind::Query, parent, span);
        self.alloc_query(
            Query {
                span,
                scope,
                ctes,
                body: Body::Hole(kind),
                order_by: Vec::new(),
                limit: None,
                outputs: Vec::new(),
            },
            true,
        )
    }

    pub(super) fn current_scope(&self) -> ScopeId {
        self.frames.last().map_or(self.root_scope, |f| f.scope)
    }

    pub(super) fn alloc_scope(&mut self, kind: ScopeKind, parent: ScopeId, span: Span) -> ScopeId {
        self.hir.scopes.alloc(Scope {
            span,
            parent: Some(parent),
            kind,
            from: Vec::new(),
        })
    }

    pub(super) fn alloc_query(&mut self, query: Query, open: bool) -> QueryId {
        let id = self.hir.queries.alloc(query);
        self.query_open.push(open);
        id
    }

    pub(super) fn alloc_from(
        &mut self,
        span: Span,
        kind: FromKind,
        output: Vec<FromCol>,
        open: bool,
        names: Vec<Symbol>,
    ) -> FromId {
        let index = (output.len() > INDEXED_WIDTH).then(|| name_index(&output));
        let id = self.hir.from.alloc(From {
            span,
            kind,
            output,
            open,
        });
        self.from_names.push(names);
        self.from_index.push(index);
        id
    }

    pub(super) fn alloc_expr(&mut self, span: Span, kind: ExprKind) -> ExprId {
        self.hir.exprs.alloc(Expr { span, kind })
    }

    /// Reserves `n` non-arena nodes (output columns, candidates) against
    /// the node cap. Callers ask before they allocate: column lists can
    /// double per CTE, so charging afterwards would be too late.
    pub(super) fn reserve(&mut self, n: usize) -> bool {
        let total = node_count(&self.hir)
            .saturating_add(self.extra_nodes)
            .saturating_add(n);
        if total > MAX_NODES {
            self.report_node_cap();
            return false;
        }
        self.extra_nodes = self.extra_nodes.saturating_add(n);
        true
    }

    /// True once the plan is at the node cap; reported once.
    pub(super) fn over_budget(&mut self) -> bool {
        let over = node_count(&self.hir).saturating_add(self.extra_nodes) > MAX_NODES;
        if over {
            self.report_node_cap();
        }
        over
    }

    fn report_node_cap(&mut self) {
        if !self.cap_reported {
            self.cap_reported = true;
            self.degraded.push(Degradation::new(
                DegradationKind::CapExceeded,
                format!("the plan exceeds {MAX_NODES} nodes; the rest stays open"),
            ));
        }
    }

    pub(super) fn degrade(&mut self, kind: DegradationKind, span: Span, detail: impl Into<String>) {
        let degradation = Degradation::new(kind, detail);
        self.degraded.push(if span.is_empty() {
            degradation
        } else {
            degradation.at(span)
        });
    }

    pub(super) fn output_col(span: Span, name: Option<Symbol>, expr: Option<ExprId>) -> OutputCol {
        OutputCol {
            span,
            name,
            expr,
            ty: cogitator_ir::ColType::UNKNOWN,
        }
    }

    /// Spans known and joined; approximate by construction (see
    /// `ExprKind::span_is_exact`).
    pub(super) fn union_spans(&self, children: &[ExprId]) -> Span {
        let mut out: Option<Span> = None;
        for child in children {
            if let Some(expr) = self.hir.exprs.get(*child) {
                if !expr.span.is_empty() {
                    out = Some(out.map_or(expr.span, |s: Span| union(s, expr.span)));
                }
            }
        }
        out.unwrap_or_default()
    }
}
