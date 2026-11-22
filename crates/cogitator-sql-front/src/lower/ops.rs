//! Operators and the fixed-shape expression forms.

use cogitator_ir::common::{BinaryOp, UnaryOp};
use cogitator_ir::hir::{ExprId, ExprKind, ScopeKind};
use sqlparser::ast::{self, Expr};

use super::Lowerer;
use crate::types::to_sql_type;

impl Lowerer<'_> {
    pub(super) fn binary_op(
        &mut self,
        left: &Expr,
        op: &ast::BinaryOperator,
        right: &Expr,
    ) -> ExprId {
        use ast::BinaryOperator as B;
        let op = match op {
            B::Eq => BinaryOp::Eq,
            B::NotEq => BinaryOp::NotEq,
            B::Lt => BinaryOp::Lt,
            B::LtEq => BinaryOp::LtEq,
            B::Gt => BinaryOp::Gt,
            B::GtEq => BinaryOp::GtEq,
            B::And => BinaryOp::And,
            B::Or => BinaryOp::Or,
            B::Plus => BinaryOp::Plus,
            B::Minus => BinaryOp::Minus,
            B::Multiply => BinaryOp::Mul,
            B::Divide => BinaryOp::Div,
            B::Modulo => BinaryOp::Mod,
            B::StringConcat => BinaryOp::Concat,
            other => BinaryOp::Other(self.intern(&other.to_string())),
        };
        self.binary(op, left, right)
    }

    pub(super) fn binary(&mut self, op: BinaryOp, left: &Expr, right: &Expr) -> ExprId {
        let l = self.expr(left);
        let r = self.expr(right);
        let span = self.union_spans(&[l, r]);
        self.alloc_expr(span, ExprKind::Binary { op, l, r })
    }

    pub(super) fn binary_named(&mut self, name: &str, a: &Expr, b: &Expr) -> ExprId {
        let op = BinaryOp::Other(self.intern(name));
        self.binary(op, a, b)
    }

    pub(super) fn quantified(
        &mut self,
        left: &Expr,
        op: &ast::BinaryOperator,
        right: &Expr,
    ) -> ExprId {
        let name = format!("{op} quantified");
        self.binary_named(&name, left, right)
    }

    pub(super) fn unary_op(&mut self, op: &ast::UnaryOperator, inner: &Expr) -> ExprId {
        use ast::UnaryOperator as U;
        match op {
            U::Not => self.unary(UnaryOp::Not, inner),
            U::Minus => self.unary(UnaryOp::Neg, inner),
            U::Plus => self.expr(inner),
            other => self.unary_named(&other.to_string(), inner),
        }
    }

    pub(super) fn unary(&mut self, op: UnaryOp, inner: &Expr) -> ExprId {
        let e = self.expr(inner);
        let span = self.union_spans(&[e]);
        self.alloc_expr(span, ExprKind::Unary { op, e })
    }

    pub(super) fn unary_named(&mut self, name: &str, inner: &Expr) -> ExprId {
        let op = UnaryOp::Other(self.intern(name));
        self.unary(op, inner)
    }

    pub(super) fn in_list(&mut self, expr: &Expr, list: &[Expr], negated: bool) -> ExprId {
        let e = self.expr(expr);
        let list: Vec<ExprId> = list.iter().map(|x| self.expr(x)).collect();
        let mut all = list.clone();
        all.push(e);
        let span = self.union_spans(&all);
        self.alloc_expr(span, ExprKind::InList { e, list, negated })
    }

    pub(super) fn in_subquery(
        &mut self,
        expr: &Expr,
        subquery: &ast::Query,
        negated: bool,
    ) -> ExprId {
        let e = self.expr(expr);
        let q = self.query(subquery, ScopeKind::Subquery, false);
        let span = self.union_spans(&[e]);
        self.alloc_expr(span, ExprKind::InQuery { e, q, negated })
    }

    pub(super) fn between(
        &mut self,
        expr: &Expr,
        low: &Expr,
        high: &Expr,
        negated: bool,
    ) -> ExprId {
        let (e, lo, hi) = (self.expr(expr), self.expr(low), self.expr(high));
        let span = self.union_spans(&[e, lo, hi]);
        self.alloc_expr(span, ExprKind::Between { e, lo, hi, negated })
    }

    pub(super) fn like(
        &mut self,
        negated: bool,
        insensitive: bool,
        expr: &Expr,
        pattern: &Expr,
        escape: Option<&Expr>,
    ) -> ExprId {
        if escape.is_some() {
            let mut children = vec![expr, pattern];
            children.extend(escape);
            return self.opaque_of(&children);
        }
        let op = match (negated, insensitive) {
            (false, false) => BinaryOp::Like,
            (true, false) => BinaryOp::NotLike,
            (false, true) => BinaryOp::ILike,
            (true, true) => BinaryOp::NotILike,
        };
        self.binary(op, expr, pattern)
    }

    pub(super) fn cast(&mut self, expr: &Expr, data_type: &ast::DataType) -> ExprId {
        let e = self.expr(expr);
        let ty = to_sql_type(data_type);
        let span = self.union_spans(&[e]);
        self.alloc_expr(span, ExprKind::Cast { e, ty })
    }

    pub(super) fn case(
        &mut self,
        operand: Option<&Expr>,
        conditions: &[ast::CaseWhen],
        else_result: Option<&Expr>,
    ) -> ExprId {
        let operand = operand.map(|e| self.expr(e));
        let arms: Vec<(ExprId, ExprId)> = conditions
            .iter()
            .map(|c| (self.expr(&c.condition), self.expr(&c.result)))
            .collect();
        let else_ = else_result.map(|e| self.expr(e));
        let mut parts: Vec<ExprId> = arms.iter().flat_map(|(w, t)| [*w, *t]).collect();
        parts.extend(operand);
        parts.extend(else_);
        let span = self.union_spans(&parts);
        self.alloc_expr(
            span,
            ExprKind::Case {
                operand,
                arms,
                else_,
            },
        )
    }
}
