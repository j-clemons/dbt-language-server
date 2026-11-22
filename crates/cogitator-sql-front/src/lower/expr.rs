//! Expressions. Anything not modelled becomes `Opaque { children }`, so
//! the column references inside it are still seen.

use cogitator_ir::common::UnaryOp;
use cogitator_ir::hir::{ExprId, ExprKind, ScopeKind};
use cogitator_ir::{DegradationKind, Span};
use sqlparser::ast::{self, Expr};

use super::Lowerer;
use crate::caps::MAX_EXPR_DEPTH;

impl Lowerer<'_> {
    /// Lowers one expression. The depth check is a backstop: the
    /// pre-parse bound normally stops these inputs from reaching here.
    pub(super) fn expr(&mut self, e: &Expr) -> ExprId {
        // `>`: a chain of MAX_EXPR_DEPTH operators has one more level of leaves.
        if self.depth > MAX_EXPR_DEPTH {
            return self.depth_hole();
        }
        if self.over_budget() {
            return self.alloc_expr(Span::default(), ExprKind::Hole);
        }
        self.depth += 1;
        let id = self.expr_inner(e);
        self.depth -= 1;
        id
    }

    fn depth_hole(&mut self) -> ExprId {
        if !self.depth_reported {
            self.depth_reported = true;
            self.degrade(
                DegradationKind::CapExceeded,
                Span::default(),
                format!("expression depth exceeds {MAX_EXPR_DEPTH}"),
            );
        }
        self.alloc_expr(Span::default(), ExprKind::Hole)
    }

    fn expr_inner(&mut self, e: &Expr) -> ExprId {
        match e {
            Expr::Identifier(ident) => self.column(std::slice::from_ref(ident)),
            Expr::CompoundIdentifier(path) => self.column(path),
            Expr::Value(value) => self.literal(value),
            Expr::BinaryOp { left, op, right } => self.binary_op(left, op, right),
            Expr::UnaryOp { op, expr } => self.unary_op(op, expr),
            Expr::Nested(inner) => self.expr(inner),
            Expr::IsNull(inner) => self.unary(UnaryOp::IsNull, inner),
            Expr::IsNotNull(inner) => self.unary(UnaryOp::IsNotNull, inner),
            Expr::IsTrue(inner) => self.unary(UnaryOp::IsTrue, inner),
            Expr::IsFalse(inner) => self.unary(UnaryOp::IsFalse, inner),
            Expr::IsNotTrue(inner) => self.unary_named("is not true", inner),
            Expr::IsNotFalse(inner) => self.unary_named("is not false", inner),
            Expr::IsUnknown(inner) => self.unary_named("is unknown", inner),
            Expr::IsNotUnknown(inner) => self.unary_named("is not unknown", inner),
            Expr::IsDistinctFrom(a, b) => self.binary_named("is distinct from", a, b),
            Expr::IsNotDistinctFrom(a, b) => self.binary_named("is not distinct from", a, b),
            Expr::InList {
                expr,
                list,
                negated,
            } => self.in_list(expr, list, *negated),
            Expr::InSubquery {
                expr,
                subquery,
                negated,
            } => self.in_subquery(expr, subquery, *negated),
            Expr::Between {
                expr,
                negated,
                low,
                high,
            } => self.between(expr, low, high, *negated),
            Expr::Like {
                negated,
                expr,
                pattern,
                escape_char,
                ..
            } => self.like(*negated, false, expr, pattern, escape_char.as_deref()),
            Expr::ILike {
                negated,
                expr,
                pattern,
                escape_char,
                ..
            } => self.like(*negated, true, expr, pattern, escape_char.as_deref()),
            Expr::RLike {
                negated,
                expr,
                pattern,
                ..
            } => {
                let name = if *negated { "not rlike" } else { "rlike" };
                self.binary_named(name, expr, pattern)
            }
            Expr::SimilarTo {
                negated,
                expr,
                pattern,
                ..
            } => {
                let name = if *negated {
                    "not similar to"
                } else {
                    "similar to"
                };
                self.binary_named(name, expr, pattern)
            }
            Expr::AnyOp {
                left,
                compare_op,
                right,
                ..
            }
            | Expr::AllOp {
                left,
                compare_op,
                right,
            } => self.quantified(left, compare_op, right),
            Expr::Cast {
                expr, data_type, ..
            } => self.cast(expr, data_type),
            Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => self.case(operand.as_deref(), conditions, else_result.as_deref()),
            Expr::Function(f) => self.function(f),
            Expr::Exists { subquery, negated } => self.exists(subquery, *negated),
            Expr::Subquery(q) => self.scalar_subquery(q),
            Expr::TypedString(ts) => self.typed_string(ts),
            Expr::Interval(interval) => self.interval(interval),
            Expr::Named { expr, .. }
            | Expr::Prefixed { value: expr, .. }
            | Expr::Collate { expr, .. } => self.expr(expr),
            Expr::AtTimeZone {
                timestamp,
                time_zone,
            } => {
                let children = [timestamp.as_ref(), time_zone.as_ref()];
                self.call_like("at_time_zone", &children)
            }
            other => self.opaque_expr(other),
        }
    }

    fn exists(&mut self, subquery: &ast::Query, negated: bool) -> ExprId {
        let q = self.query(subquery, ScopeKind::Subquery, false);
        let span = self.hir.queries.get(q).map_or(Span::default(), |x| x.span);
        let id = self.alloc_expr(span, ExprKind::Exists(q));
        if negated {
            self.alloc_expr(
                span,
                ExprKind::Unary {
                    op: UnaryOp::Not,
                    e: id,
                },
            )
        } else {
            id
        }
    }

    pub(super) fn scalar_subquery(&mut self, subquery: &ast::Query) -> ExprId {
        let q = self.query(subquery, ScopeKind::Subquery, false);
        let span = self.hir.queries.get(q).map_or(Span::default(), |x| x.span);
        self.alloc_expr(span, ExprKind::Scalar(q))
    }

    /// `name(children...)` for SQL's keyword-call forms.
    pub(super) fn call_like(&mut self, name: &str, children: &[&Expr]) -> ExprId {
        let args: Vec<ExprId> = children.iter().map(|e| self.expr(e)).collect();
        let span = self.union_spans(&args);
        let name = self.intern(name);
        self.alloc_expr(
            span,
            ExprKind::Call {
                name,
                args,
                distinct: false,
                filter: None,
                over: None,
            },
        )
    }

    /// `Opaque { children }`, never silently nothing.
    pub(super) fn opaque_of(&mut self, children: &[&Expr]) -> ExprId {
        let children: Vec<ExprId> = children.iter().map(|e| self.expr(e)).collect();
        let span = self.union_spans(&children);
        self.alloc_expr(span, ExprKind::Opaque { children })
    }
}
