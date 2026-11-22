//! Scalar expressions. Every node keeps provenance to its HIR expression
//! and a type from `infer`.

use cogitator_ir::common::{BinaryOp, HoleKind};
use cogitator_ir::hir::{ExprId, ExprKind, QueryId};
use cogitator_ir::mir::{HirNode, ProvId, ScalarId, ScalarKind, SubqueryKind};
use cogitator_ir::{ColType, DegradationKind, Nullability, Span, SqlType, Symbol};

use super::equal::same_expr;
use super::{Ctx, MAX_EXPR_DEPTH};
use crate::infer;

impl Ctx<'_> {
    pub(super) fn expr(&mut self, id: ExprId) -> ScalarId {
        let hir = self.hir;
        let Some(node) = hir.exprs.get(id) else {
            self.bug(format!("expression {id:?} is out of range"));
            let prov = self.prov(HirNode::None, Span::default());
            return self.opaque_scalar(Vec::new(), HoleKind::Unsupported, Span::default(), prov);
        };
        let span = node.span;
        let prov = self.prov(HirNode::Expr(id), span);
        if self.expr_depth >= MAX_EXPR_DEPTH || !self.tick() {
            self.cap(span, "this expression");
            return self.opaque_scalar(Vec::new(), HoleKind::Truncated, span, prov);
        }
        if let Some(scalar) = self.replaced(id, prov) {
            return scalar;
        }
        self.expr_depth += 1;
        let scalar = self.kind(id, &node.kind, span, prov);
        self.expr_depth -= 1;
        scalar
    }

    /// Group keys, aggregates and window calls were computed below;
    /// above their operator they read as that operator's column.
    fn replaced(&mut self, id: ExprId, prov: ProvId) -> Option<ScalarId> {
        let (col, ty) = if let Some(col) = self.env.wins.get(&id).or(self.env.aggs.get(&id)) {
            (*col, self.col_type(*col))
        } else if self.env.grouped {
            let hir = self.hir;
            let (_, col, ty) = self
                .env
                .keys
                .iter()
                .find(|(key, _, _)| same_expr(hir, *key, id))?;
            (*col, ty.clone())
        } else {
            return None;
        };
        Some(self.col_scalar(col, ty, prov))
    }

    fn kind(&mut self, id: ExprId, kind: &ExprKind, span: Span, prov: ProvId) -> ScalarId {
        match kind {
            ExprKind::Col(col) => self.column(id, *col, span),
            ExprKind::Lit(lit) => {
                let syms = &self.mir.syms;
                let ty = infer::literal(lit, |s| syms.text(s).to_string());
                self.mir.add_scalar(ScalarKind::Lit(*lit), ty, prov)
            }
            ExprKind::Unary { op, e } => {
                let e = self.expr(*e);
                let ty = infer::unary(*op, &self.scalar_type(e));
                self.mir
                    .add_scalar(ScalarKind::Unary { op: *op, e }, ty, prov)
            }
            ExprKind::Binary { op, l, r } => {
                let (l, r) = (self.expr(*l), self.expr(*r));
                self.binary(*op, l, r, prov)
            }
            ExprKind::Call {
                name,
                args,
                distinct,
                filter,
                ..
            } => {
                if filter.is_some() {
                    self.degrade(
                        DegradationKind::UnsupportedSyntax,
                        span,
                        "FILTER on a non-aggregate call is not represented",
                    );
                }
                self.call(*name, args, *distinct, prov)
            }
            ExprKind::Case {
                operand,
                arms,
                else_,
            } => self.case(*operand, arms, *else_, prov),
            ExprKind::Cast { e, ty } => {
                let e = self.expr(*e);
                let null = self.scalar_type(e).null;
                self.mir.add_scalar(
                    ScalarKind::Cast { e, ty: ty.clone() },
                    ColType::new(ty.clone(), null),
                    prov,
                )
            }
            ExprKind::InList { e, list, negated } => {
                let e = self.expr(*e);
                let list = list.iter().map(|x| self.expr(*x)).collect();
                let negated = *negated;
                self.mir.add_scalar(
                    ScalarKind::InList { e, list, negated },
                    ColType::new(SqlType::Bool, Nullability::Unknown),
                    prov,
                )
            }
            ExprKind::InQuery { e, q, negated } => {
                let e = self.expr(*e);
                let kind = SubqueryKind::In {
                    e,
                    negated: *negated,
                };
                let ty = ColType::new(SqlType::Bool, Nullability::Unknown);
                self.subquery(*q, kind, ty, prov)
            }
            ExprKind::Between { e, lo, hi, negated } => self.between(*e, *lo, *hi, *negated, prov),
            ExprKind::Exists(q) => {
                let ty = ColType::new(SqlType::Bool, Nullability::NotNull);
                self.subquery(*q, SubqueryKind::Exists, ty, prov)
            }
            ExprKind::Scalar(q) => self.subquery(*q, SubqueryKind::Scalar, ColType::UNKNOWN, prov),
            ExprKind::Jinja(_) => {
                self.degrade(
                    DegradationKind::OpaqueExpression,
                    span,
                    "a Jinja expression has no static value",
                );
                self.opaque_scalar(Vec::new(), HoleKind::Jinja, span, prov)
            }
            ExprKind::Opaque { children } => {
                self.degrade(
                    DegradationKind::OpaqueExpression,
                    span,
                    "this expression is not modelled",
                );
                let children = children.iter().map(|c| self.expr(*c)).collect();
                self.opaque_scalar(children, HoleKind::Unsupported, span, prov)
            }
            ExprKind::Hole => {
                self.degrade(
                    DegradationKind::ParseHole,
                    span,
                    "this expression did not parse",
                );
                self.opaque_scalar(Vec::new(), HoleKind::ParseError, span, prov)
            }
        }
    }

    pub(super) fn scalar_type(&self, id: ScalarId) -> ColType {
        self.mir
            .scalars
            .get(id)
            .map_or(ColType::UNKNOWN, |s| s.ty.clone())
    }

    fn binary(&mut self, op: BinaryOp, l: ScalarId, r: ScalarId, prov: ProvId) -> ScalarId {
        let ty = infer::binary(op, &self.scalar_type(l), &self.scalar_type(r));
        self.mir
            .add_scalar(ScalarKind::Binary { op, l, r }, ty, prov)
    }

    pub(super) fn call(
        &mut self,
        name: Symbol,
        args: &[ExprId],
        distinct: bool,
        prov: ProvId,
    ) -> ScalarId {
        let args: Vec<ScalarId> = args.iter().map(|a| self.expr(*a)).collect();
        let types: Vec<ColType> = args.iter().map(|a| self.scalar_type(*a)).collect();
        let ty = infer::call(self.mir.syms.text(name), &types);
        self.mir.add_scalar(
            ScalarKind::Call {
                name,
                args,
                distinct,
            },
            ty,
            prov,
        )
    }

    fn case(
        &mut self,
        operand: Option<ExprId>,
        arms: &[(ExprId, ExprId)],
        else_: Option<ExprId>,
        prov: ProvId,
    ) -> ScalarId {
        let operand = operand.map(|o| self.expr(o));
        let arms: Vec<(ScalarId, ScalarId)> = arms
            .iter()
            .map(|(w, t)| (self.expr(*w), self.expr(*t)))
            .collect();
        let else_ = else_.map(|e| self.expr(e));
        let mut results: Vec<ColType> = arms.iter().map(|(_, t)| self.scalar_type(*t)).collect();
        results.extend(else_.iter().map(|e| self.scalar_type(*e)));
        let ty = infer::case(&results, else_.is_some());
        self.mir.add_scalar(
            ScalarKind::Case {
                operand,
                arms,
                else_,
            },
            ty,
            prov,
        )
    }

    /// `BETWEEN` is two comparisons joined by `AND`, which the conjunct
    /// splitter then separates. `e` is lowered once per comparison so
    /// scalars stay a tree.
    fn between(
        &mut self,
        e: ExprId,
        lo: ExprId,
        hi: ExprId,
        negated: bool,
        prov: ProvId,
    ) -> ScalarId {
        let (low, high, join) = if negated {
            (BinaryOp::Lt, BinaryOp::Gt, BinaryOp::Or)
        } else {
            (BinaryOp::GtEq, BinaryOp::LtEq, BinaryOp::And)
        };
        let (e1, lo) = (self.expr(e), self.expr(lo));
        let lower = self.binary(low, e1, lo, prov);
        let (e2, hi) = (self.expr(e), self.expr(hi));
        let upper = self.binary(high, e2, hi, prov);
        self.binary(join, lower, upper, prov)
    }

    fn subquery(&mut self, q: QueryId, kind: SubqueryKind, ty: ColType, prov: ProvId) -> ScalarId {
        self.frames.push(Vec::new());
        let out = self.query(q, &[]);
        let correlated = self.frames.pop().unwrap_or_default();
        let ty = match kind {
            SubqueryKind::Scalar if !out.open => out.cols.first().map_or(ColType::UNKNOWN, |c| {
                ColType::new(self.col_type(*c).ty, Nullability::Unknown)
            }),
            _ => ty,
        };
        self.mir.add_scalar(
            ScalarKind::Subquery {
                kind,
                rel: out.rel,
                correlated,
            },
            ty,
            prov,
        )
    }

    /// Lowers `e` and splits it at top-level `AND`s, in source order.
    pub(super) fn conjuncts(&mut self, e: ExprId) -> Vec<ScalarId> {
        let root = self.expr(e);
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            match self.mir.scalars.get(id).map(|s| &s.kind) {
                Some(ScalarKind::Binary {
                    op: BinaryOp::And,
                    l,
                    r,
                }) => {
                    stack.push(*r);
                    stack.push(*l);
                }
                _ => out.push(id),
            }
        }
        out
    }
}
