//! Set operations: arms, flags and the combined outputs.

use cogitator_ir::common::SetOpKind;
use cogitator_ir::hir::{Body, OutputCol, Query, QueryId, ScopeKind};
use cogitator_ir::Span;
use sqlparser::ast::{SetExpr, SetOperator, SetQuantifier};

use super::binder::{AliasMode, Frame};
use super::query::Lowered;
use super::Lowerer;

impl Lowerer<'_> {
    pub(super) fn set_operation(
        &mut self,
        left: &SetExpr,
        op: SetOperator,
        quantifier: SetQuantifier,
        right: &SetExpr,
    ) -> Lowered {
        let l = self.set_arm(left);
        let r = self.set_arm(right);
        let (all, by_name) = quantifier_flags(quantifier);
        let op = match op {
            SetOperator::Union => SetOpKind::Union,
            SetOperator::Intersect => SetOpKind::Intersect,
            SetOperator::Except | SetOperator::Minus => SetOpKind::Except,
        };
        let (outputs, open) = self.set_outputs(l, r, by_name);
        let span = self.arms_span(l, r);
        Lowered {
            body: Body::SetOp {
                op,
                all,
                by_name,
                l,
                r,
            },
            outputs,
            open,
            span,
        }
    }

    pub(super) fn arms_span(&self, l: QueryId, r: QueryId) -> Span {
        match (self.hir.queries.get(l), self.hir.queries.get(r)) {
            (Some(a), Some(b)) => super::union(a.span, b.span),
            _ => Span::default(),
        }
    }

    /// One arm: a parenthesised query keeps its clauses, a bare SELECT
    /// is wrapped in its own query node and scope.
    pub(super) fn set_arm(&mut self, arm: &SetExpr) -> QueryId {
        match arm {
            SetExpr::Query(q) => self.query(q, ScopeKind::Query, false),
            other => {
                let parent = self.current_scope();
                let scope = self.alloc_scope(ScopeKind::Query, parent, Span::default());
                self.frames.push(Frame::new(scope, false));
                let saved = std::mem::replace(&mut self.mode, AliasMode::Off);
                let lowered = self.set_expr_body(other);
                self.finish_scope(scope, lowered.span);
                self.frames.pop();
                self.mode = saved;
                self.alloc_query(
                    Query {
                        span: lowered.span,
                        scope,
                        ctes: Vec::new(),
                        body: lowered.body,
                        order_by: Vec::new(),
                        limit: None,
                        outputs: lowered.outputs,
                    },
                    lowered.open,
                )
            }
        }
    }

    /// The left arm's columns; `BY NAME` appends right-only names.
    pub(super) fn set_outputs(
        &mut self,
        l: QueryId,
        r: QueryId,
        by_name: bool,
    ) -> (Vec<OutputCol>, bool) {
        let open = self.query_is_open(l) || self.query_is_open(r);
        let width = |q: QueryId| self.hir.queries.get(q).map_or(0, |x| x.outputs.len());
        let wanted = width(l) + if by_name { width(r) } else { 0 };
        if !self.reserve(wanted) {
            return (Vec::new(), true);
        }
        let Some(left) = self.hir.queries.get(l) else {
            return (Vec::new(), true);
        };
        // boundary: outputs are owned per query; arms and the set op differ
        let mut outputs = left.outputs.clone();
        if let (true, Some(right)) = (by_name, self.hir.queries.get(r)) {
            for col in &right.outputs {
                let present = col.name.is_some() && outputs.iter().any(|o| o.name == col.name);
                if !present {
                    outputs.push(col.clone());
                }
            }
        }
        (outputs, open)
    }
}

fn quantifier_flags(q: SetQuantifier) -> (bool, bool) {
    match q {
        SetQuantifier::All => (true, false),
        SetQuantifier::Distinct | SetQuantifier::None => (false, false),
        SetQuantifier::ByName | SetQuantifier::DistinctByName => (false, true),
        SetQuantifier::AllByName => (true, true),
    }
}
