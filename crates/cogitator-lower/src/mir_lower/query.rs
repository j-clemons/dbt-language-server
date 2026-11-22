//! Queries: dispatch over the body, holes, correlation isolation and
//! LIMIT.

use cogitator_ir::common::HoleKind;
use cogitator_ir::hir::{Body, ExprId, ExprKind, Query, QueryId};
use cogitator_ir::mir::{ColId, HirNode, ProvId, RelId, RelOp, ScalarId};
use cogitator_ir::{ColType, DegradationKind, Span, Symbol};

use super::{Ctx, OpaqueSpec, MAX_QUERY_DEPTH};

/// A lowered query. `cols` is exactly `rel`'s output.
pub(super) struct QueryOut {
    pub(super) rel: RelId,
    pub(super) cols: Vec<ColId>,
    /// More columns may exist than `cols` names.
    pub(super) open: bool,
}

impl Ctx<'_> {
    pub(super) fn query(&mut self, q: QueryId, demand: &[Symbol]) -> QueryOut {
        let hir = self.hir;
        let Some(query) = hir.queries.get(q) else {
            self.bug(format!("query {q:?} is out of range"));
            let prov = self.prov(HirNode::None, Span::default());
            return self.hole(demand, HoleKind::Unsupported, Span::default(), prov);
        };
        let prov = self.prov(HirNode::Query(q), query.span);
        if self.query_depth >= MAX_QUERY_DEPTH || !self.tick() {
            self.cap(query.span, "this query");
            return self.hole(demand, HoleKind::Truncated, query.span, prov);
        }
        self.query_depth += 1;
        for cte in &query.ctes {
            self.cte(*cte);
        }
        let out = match &query.body {
            Body::Select(select) => self.select(q, query, select, demand),
            Body::SetOp {
                op,
                all,
                by_name,
                l,
                r,
            } => {
                let arms = self.flatten_set_op(*op, *all, *by_name, *l, *r);
                let out = self.set_op(*op, *all, *by_name, &arms, demand, query, prov);
                self.order_outputs(query, out, prov)
            }
            Body::Values(rows) => {
                let out = self.values(rows, query, demand, prov);
                self.order_outputs(query, out, prov)
            }
            Body::Hole(kind) => {
                self.degrade(
                    hole_degradation(*kind),
                    query.span,
                    "this query did not parse",
                );
                let out = self.hole(demand, *kind, query.span, prov);
                self.order_outputs(query, out, prov)
            }
        };
        let out = self.limit(query, out, prov);
        self.query_depth -= 1;
        out
    }

    /// An `Opaque` with no inputs, exposing only demanded columns.
    pub(super) fn hole(
        &mut self,
        demand: &[Symbol],
        kind: HoleKind,
        span: Span,
        prov: ProvId,
    ) -> QueryOut {
        let spec = OpaqueSpec {
            cols: demand
                .iter()
                .map(|n| (Some(*n), ColType::UNKNOWN))
                .collect(),
            open: true,
            kind,
            span,
        };
        let (rel, cols) = self.opaque_rel(Vec::new(), spec, prov);
        QueryOut {
            rel,
            cols,
            open: true,
        }
    }

    /// Runs `q` in its own correlation frame. A body that reads outward
    /// cannot be a plan input (the verifier would see an unbound column),
    /// so it becomes an `Opaque` with the same columns.
    pub(super) fn isolated(
        &mut self,
        q: QueryId,
        demand: &[Symbol],
        span: Span,
        prov: ProvId,
    ) -> QueryOut {
        self.frames.push(Vec::new());
        let out = self.query(q, demand);
        let captured = self.frames.pop().unwrap_or_default();
        if captured.is_empty() {
            return out;
        }
        self.degrade(
            DegradationKind::UnsupportedSyntax,
            span,
            "a correlated CTE or LATERAL body is not modelled",
        );
        let spec = OpaqueSpec {
            cols: self.spec_of(&out.cols),
            open: out.open,
            kind: HoleKind::Unsupported,
            span,
        };
        let (rel, cols) = self.opaque_rel(Vec::new(), spec, prov);
        QueryOut {
            rel,
            cols,
            open: out.open,
        }
    }

    pub(super) fn spec_of(&self, cols: &[ColId]) -> Vec<(Option<Symbol>, ColType)> {
        cols.iter()
            .map(|c| (self.col_name(*c), self.col_type(*c)))
            .collect()
    }

    fn limit(&mut self, query: &Query, out: QueryOut, prov: ProvId) -> QueryOut {
        let Some(expr) = query.limit else {
            return out;
        };
        let scalar = self.limit_scalar(expr);
        let rel = self.mir.add_rel(
            RelOp::Limit {
                input: out.rel,
                limit: Some(scalar),
                offset: None,
            },
            prov,
        );
        QueryOut { rel, ..out }
    }

    /// A LIMIT cannot read columns; a reference is opaque, not a violation.
    fn limit_scalar(&mut self, expr: ExprId) -> ScalarId {
        let hir = self.hir;
        let not_literal = hir.exprs.get(expr).is_some_and(|e| {
            !matches!(
                e.kind,
                ExprKind::Lit(_) | ExprKind::Jinja(_) | ExprKind::Hole
            )
        });
        if not_literal {
            let span = hir.exprs.get(expr).map_or(Span::default(), |e| e.span);
            let prov = self.prov(HirNode::Expr(expr), span);
            self.degrade(
                DegradationKind::OpaqueExpression,
                span,
                "this LIMIT is not a literal",
            );
            return self.opaque_scalar(Vec::new(), HoleKind::Unsupported, span, prov);
        }
        self.expr(expr)
    }
}

fn hole_degradation(kind: HoleKind) -> DegradationKind {
    match kind {
        HoleKind::ParseError => DegradationKind::ParseHole,
        HoleKind::Unsupported => DegradationKind::UnsupportedSyntax,
        HoleKind::Jinja => DegradationKind::JinjaControlFlow,
        HoleKind::Truncated => DegradationKind::CapExceeded,
    }
}
