//! Window functions. Each `OVER` call becomes a column of one `Window`
//! node below the final projection; named windows are resolved here.

use cogitator_ir::hir::{ExprId, ExprKind, OrderItem, Select, WindowSpec};
use cogitator_ir::mir::{ColId, ProvId, RelId, RelOp, ScalarId, SortKey, WinCall};
use cogitator_ir::{ColType, DegradationKind};

use super::Ctx;
use crate::infer;

impl Ctx<'_> {
    pub(super) fn window(
        &mut self,
        input: RelId,
        select: &Select,
        wins: &[ExprId],
        prov: ProvId,
    ) -> RelId {
        let funcs: Vec<(ColId, WinCall)> = wins
            .iter()
            .filter_map(|id| self.win_call(*id, select))
            .collect();
        if funcs.is_empty() {
            return input;
        }
        for (id, (col, _)) in wins.iter().zip(&funcs) {
            self.env.wins.insert(*id, *col);
        }
        self.mir.add_rel(RelOp::Window { input, funcs }, prov)
    }

    fn win_call(&mut self, id: ExprId, select: &Select) -> Option<(ColId, WinCall)> {
        let hir = self.hir;
        let node = hir.exprs.get(id)?;
        let ExprKind::Call {
            name,
            args,
            distinct,
            filter,
            over: Some(spec),
        } = &node.kind
        else {
            return None;
        };
        if filter.is_some() || *distinct {
            self.degrade(
                DegradationKind::UnsupportedSyntax,
                node.span,
                "FILTER or DISTINCT on a window call is not represented",
            );
        }
        let args: Vec<ScalarId> = args.iter().map(|a| self.expr(*a)).collect();
        let named = spec
            .name
            .and_then(|w| select.windows.iter().find(|(n, _)| n.folded == w.folded));
        let effective: &WindowSpec = named.map_or(spec, |(_, s)| s);
        let partition_by = effective
            .partition_by
            .iter()
            .map(|e| self.expr(*e))
            .collect();
        let order_by = effective
            .order_by
            .iter()
            .map(|k| self.sort_key(k))
            .collect();
        let types: Vec<ColType> = args.iter().map(|a| self.scalar_type(*a)).collect();
        let ty = infer::window(hir.syms.text(*name), &types);
        let col = self.mir.add_col(None, ty);
        Some((
            col,
            WinCall {
                func: *name,
                args,
                partition_by,
                order_by,
                frame: spec.frame.or(effective.frame),
            },
        ))
    }

    fn sort_key(&mut self, key: &OrderItem) -> SortKey {
        SortKey {
            expr: self.expr(key.expr),
            desc: key.desc,
            nulls_first: key.nulls_first,
        }
    }
}
