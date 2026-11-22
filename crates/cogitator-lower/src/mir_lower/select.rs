//! One SELECT: FROM → WHERE → Aggregate → HAVING → Window → QUALIFY,
//! then the output steps in `order`. HAVING and QUALIFY are `Filter`s.

use cogitator_ir::hir::{ExprId, Query, QueryId, Select};
use cogitator_ir::mir::{HirNode, ProvId, RelId, RelOp};
use cogitator_ir::Symbol;

use super::query::QueryOut;
use super::Ctx;

impl Ctx<'_> {
    pub(super) fn select(
        &mut self,
        q: QueryId,
        query: &Query,
        select: &Select,
        demand: &[Symbol],
    ) -> QueryOut {
        let saved = std::mem::take(&mut self.env);
        let prov = self.prov(HirNode::Query(q), select.span);
        let out = self.select_inner(query, select, demand, prov);
        self.env = saved;
        out
    }

    fn select_inner(
        &mut self,
        query: &Query,
        select: &Select,
        demand: &[Symbol],
        prov: ProvId,
    ) -> QueryOut {
        let mut rel = self.lower_from_list(&select.from, prov);
        let expanded = self.expand_items(select);
        self.env.aliases = select.items.iter().map(alias_expr).collect();
        rel = self.filter(rel, select.filter, prov);
        let calls = self.collect_calls(query, select, &expanded.items);
        if self.needs_aggregate(select, &calls.aggs) {
            rel = self.aggregate(rel, select, &expanded.items, &calls.aggs, prov);
        }
        rel = self.filter(rel, select.having, prov);
        rel = self.window(rel, select, &calls.wins, prov);
        rel = self.filter(rel, select.qualify, prov);
        self.finish_select(query, select, expanded, rel, demand, prov)
    }

    fn filter(&mut self, input: RelId, expr: Option<ExprId>, prov: ProvId) -> RelId {
        let Some(expr) = expr else {
            return input;
        };
        let conjuncts = self.conjuncts(expr);
        self.mir.add_rel(RelOp::Filter { input, conjuncts }, prov)
    }
}

fn alias_expr(item: &cogitator_ir::hir::SelectItem) -> Option<ExprId> {
    match item {
        cogitator_ir::hir::SelectItem::Expr { expr, .. } => Some(*expr),
        _ => None,
    }
}
