//! Set operations, VALUES, and ORDER BY over named outputs.

use cogitator_ir::common::{HoleKind, Lit, SetOpKind};
use cogitator_ir::hir::{Body, ExprId, ExprKind, Query, QueryId, Resolution};
use cogitator_ir::mir::{ColId, ProvId, RelId, RelOp, SortKey};
use cogitator_ir::{ColType, DegradationKind, Symbol};

use super::query::QueryOut;
use super::{Ctx, OpaqueSpec};

impl Ctx<'_> {
    /// `a UNION ALL b UNION ALL c` as one n-ary list, so long chains do
    /// not consume query depth. Only a bare left arm is absorbed.
    pub(super) fn flatten_set_op(
        &self,
        op: SetOpKind,
        all: bool,
        by_name: bool,
        l: QueryId,
        r: QueryId,
    ) -> Vec<QueryId> {
        let hir = self.hir;
        let mut arms = vec![r];
        let mut left = l;
        while let Some(query) = hir.queries.get(left) {
            match &query.body {
                Body::SetOp {
                    op: inner,
                    all: inner_all,
                    by_name: inner_name,
                    l,
                    r,
                } if *inner == op
                    && *inner_all == all
                    && *inner_name == by_name
                    && query.ctes.is_empty()
                    && query.order_by.is_empty()
                    && query.limit.is_none()
                    && arms.len() < hir.queries.len() =>
                {
                    arms.push(*r);
                    left = *l;
                }
                _ => break,
            }
        }
        arms.push(left);
        arms.reverse();
        arms
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn set_op(
        &mut self,
        op: SetOpKind,
        all: bool,
        by_name: bool,
        arms: &[QueryId],
        demand: &[Symbol],
        query: &Query,
        prov: ProvId,
    ) -> QueryOut {
        let lowered: Vec<QueryOut> = arms.iter().map(|q| self.query(*q, demand)).collect();
        let inputs: Vec<RelId> = lowered.iter().map(|a| a.rel).collect();
        let first = lowered.first().map_or(&[][..], |a| a.cols.as_slice());
        let same_width = lowered.iter().all(|a| a.cols.len() == first.len());
        let open = lowered.iter().any(|a| a.open);
        let names_align = !by_name
            || lowered.iter().all(|a| {
                a.cols
                    .iter()
                    .zip(first)
                    .all(|(x, y)| self.col_name(*x) == self.col_name(*y))
            });
        if open || !same_width || !names_align {
            let span = query.span;
            let (kind, why) = if open {
                (
                    DegradationKind::OpenSchema,
                    "a set operation over an open arm",
                )
            } else {
                (
                    DegradationKind::UnsupportedSyntax,
                    "set operation arms do not line up by position",
                )
            };
            self.degrade(kind, span, why);
            let spec = OpaqueSpec {
                cols: self.spec_of(first),
                open: true,
                kind: HoleKind::Unsupported,
                span,
            };
            let (rel, cols) = self.opaque_rel(inputs, spec, prov);
            return QueryOut {
                rel,
                cols,
                open: true,
            };
        }
        let out: Vec<ColId> = (0..first.len())
            .map(|i| {
                let ty = lowered
                    .iter()
                    .map(|a| self.col_type(a.cols[i]))
                    .reduce(|acc, t| acc.join(&t))
                    .unwrap_or(ColType::UNKNOWN);
                let name = self.col_name(first[i]);
                self.mir.add_col(name, ty)
            })
            .collect();
        let rel = self.mir.add_rel(
            RelOp::SetOp {
                op,
                all,
                inputs,
                out: out.clone(),
            },
            prov,
        );
        QueryOut {
            rel,
            cols: out,
            open: false,
        }
    }

    /// One Project per row over the single-row source, unioned.
    pub(super) fn values(
        &mut self,
        rows: &[Vec<ExprId>],
        query: &Query,
        demand: &[Symbol],
        prov: ProvId,
    ) -> QueryOut {
        let width = rows.first().map_or(0, Vec::len);
        if width == 0 || rows.iter().any(|r| r.len() != width) {
            self.degrade(
                DegradationKind::UnsupportedSyntax,
                query.span,
                "this VALUES list has no consistent shape",
            );
            return self.hole(demand, HoleKind::Unsupported, query.span, prov);
        }
        let name = |i: usize| query.outputs.get(i).and_then(|o| o.name);
        let mut arms = Vec::with_capacity(rows.len());
        for row in rows {
            let input = self.singleton(prov);
            let mut items = Vec::with_capacity(width);
            for (i, expr) in row.iter().enumerate() {
                let scalar = self.expr(*expr);
                let ty = self.scalar_type(scalar);
                let col = self.mir.add_col(name(i), ty);
                items.push((col, scalar));
            }
            let cols: Vec<ColId> = items.iter().map(|(c, _)| *c).collect();
            let rel = self.mir.add_rel(RelOp::Project { input, items }, prov);
            arms.push(QueryOut {
                rel,
                cols,
                open: false,
            });
        }
        if arms.len() == 1 {
            if let Some(only) = arms.pop() {
                return only;
            }
        }
        let inputs: Vec<RelId> = arms.iter().map(|a| a.rel).collect();
        let out: Vec<ColId> = (0..width)
            .map(|i| {
                let ty = arms
                    .iter()
                    .map(|a| self.col_type(a.cols[i]))
                    .reduce(|acc, t| acc.join(&t))
                    .unwrap_or(ColType::UNKNOWN);
                self.mir.add_col(name(i), ty)
            })
            .collect();
        let rel = self.mir.add_rel(
            RelOp::SetOp {
                op: SetOpKind::Union,
                all: true,
                inputs,
                out: out.clone(),
            },
            prov,
        );
        QueryOut {
            rel,
            cols: out,
            open: false,
        }
    }

    /// ORDER BY over named outputs only: ordinals and output names.
    pub(super) fn order_outputs(&mut self, query: &Query, out: QueryOut, prov: ProvId) -> QueryOut {
        if query.order_by.is_empty() {
            return out;
        }
        let mut keys = Vec::with_capacity(query.order_by.len());
        for key in &query.order_by {
            match self.output_position(key.expr, &out.cols) {
                Some(col) => {
                    let ty = self.col_type(col);
                    keys.push(SortKey {
                        expr: self.col_scalar(col, ty, prov),
                        desc: key.desc,
                        nulls_first: key.nulls_first,
                    });
                }
                None => {
                    let span = self.hir.exprs.get(key.expr).map_or(query.span, |e| e.span);
                    self.degrade(
                        DegradationKind::UnsupportedSyntax,
                        span,
                        "this ORDER BY key does not name an output column",
                    );
                }
            }
        }
        if keys.is_empty() {
            return out;
        }
        let rel = self.mir.add_rel(
            RelOp::Sort {
                input: out.rel,
                keys,
            },
            prov,
        );
        QueryOut { rel, ..out }
    }

    fn output_position(&self, key: ExprId, cols: &[ColId]) -> Option<ColId> {
        let hir = self.hir;
        match &hir.exprs.get(key)?.kind {
            ExprKind::Lit(Lit::Number(sym)) => {
                let n = hir.syms.text(*sym).parse::<usize>().ok()?.checked_sub(1)?;
                cols.get(n).copied()
            }
            ExprKind::Col(col) => {
                let col = hir.cols.get(*col)?;
                if let Some(Resolution::LateralAlias(i)) = col.res {
                    return cols.get(i as usize).copied();
                }
                let name = col.name.folded;
                cols.iter()
                    .copied()
                    .find(|c| self.col_name(*c) == Some(name))
            }
            _ => None,
        }
    }
}
