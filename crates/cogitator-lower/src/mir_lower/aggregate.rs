//! GROUP BY and aggregate calls. Keys resolve ordinals and aliases, and
//! a bare column key passes through under its own `ColId`.

use std::collections::BTreeSet;

use cogitator_ir::common::Lit;
use cogitator_ir::hir::{ExprId, ExprKind, GroupBy, Query, Resolution, Select};
use cogitator_ir::mir::{AggCall, ColId, GroupingSets, ProvId, RelId, RelOp, ScalarId, ScalarKind};
use cogitator_ir::{ColType, DegradationKind, Symbol};

use super::equal::same_expr;
use super::star::{Field, Item};
use super::Ctx;
use crate::infer;

/// CUBE expands to 2^n sets; past this the sets are not enumerated.
const MAX_CUBE_KEYS: usize = 12;

/// HIR calls that need an operator below the final projection.
#[derive(Default)]
pub(super) struct Calls {
    pub(super) aggs: Vec<ExprId>,
    pub(super) wins: Vec<ExprId>,
}

/// One normalised GROUP BY key.
enum Key {
    Expr(ExprId),
    Field(Field),
}

impl Ctx<'_> {
    pub(super) fn collect_calls(&self, query: &Query, select: &Select, items: &[Item]) -> Calls {
        let mut calls = Calls::default();
        for item in items {
            if let Item::Expr { expr, .. } = item {
                self.walk_calls(*expr, 0, &mut calls);
            }
        }
        for expr in select.having.iter().chain(&select.qualify) {
            self.walk_calls(*expr, 0, &mut calls);
        }
        for key in &query.order_by {
            self.walk_calls(key.expr, 0, &mut calls);
        }
        calls
    }

    fn walk_calls(&self, id: ExprId, depth: usize, calls: &mut Calls) {
        if depth > super::MAX_EXPR_DEPTH {
            return;
        }
        let hir = self.hir;
        let Some(node) = hir.exprs.get(id) else {
            return;
        };
        if let ExprKind::Call { name, over, .. } = &node.kind {
            if over.is_some() {
                if !calls.wins.contains(&id) {
                    calls.wins.push(id);
                }
            } else if infer::is_aggregate(hir.syms.text(*name)) {
                if !calls.aggs.contains(&id) {
                    calls.aggs.push(id);
                }
                return;
            }
        }
        for child in node.kind.children() {
            self.walk_calls(child, depth + 1, calls);
        }
    }

    pub(super) fn needs_aggregate(&self, select: &Select, aggs: &[ExprId]) -> bool {
        select.group_by != GroupBy::None || !aggs.is_empty()
    }

    pub(super) fn aggregate(
        &mut self,
        input: RelId,
        select: &Select,
        items: &[Item],
        aggs: &[ExprId],
        prov: ProvId,
    ) -> RelId {
        let (groups, sets_kind) = self.group_lists(select, items);
        let mut keys: Vec<(ColId, ScalarId)> = Vec::new();
        let mut key_of_group: Vec<Vec<ColId>> = Vec::with_capacity(groups.len());
        let mut used = BTreeSet::new();
        for group in &groups {
            let mut cols = Vec::with_capacity(group.len());
            for key in group {
                let col = self.key_column(key, items, &mut keys, &mut used, prov);
                if !cols.contains(&col) {
                    cols.push(col);
                }
            }
            key_of_group.push(cols);
        }
        let agg_cols: Vec<(ColId, AggCall)> =
            aggs.iter().filter_map(|id| self.agg_call(*id)).collect();
        let sets = sets_kind.then_some(GroupingSets(key_of_group));
        self.env.grouped = true;
        self.mir.add_rel(
            RelOp::Aggregate {
                input,
                keys,
                aggs: agg_cols,
                sets,
            },
            prov,
        )
    }

    /// The grouping lists, and whether they are real GROUPING SETS.
    fn group_lists(&mut self, select: &Select, items: &[Item]) -> (Vec<Vec<Key>>, bool) {
        let one = |cx: &mut Self, list: &[ExprId]| -> Vec<Key> {
            list.iter().map(|e| cx.normalise_key(*e, items)).collect()
        };
        match &select.group_by {
            GroupBy::None => (vec![Vec::new()], false),
            GroupBy::Exprs(list) => (vec![one(self, list)], false),
            GroupBy::All => (vec![self.group_all(items)], false),
            GroupBy::Sets(sets) => (sets.iter().map(|s| one(self, s)).collect(), true),
            GroupBy::Rollup(list) => {
                let groups = (0..=list.len())
                    .rev()
                    .map(|n| one(self, &list[..n]))
                    .collect();
                (groups, true)
            }
            GroupBy::Cube(list) if list.len() > MAX_CUBE_KEYS => {
                self.degrade(
                    DegradationKind::UnsupportedSyntax,
                    select.span,
                    "this CUBE is too wide to enumerate its grouping sets",
                );
                (vec![one(self, list)], false)
            }
            GroupBy::Cube(list) => {
                let groups = (0..(1usize << list.len()))
                    .map(|mask| {
                        let subset: Vec<ExprId> = list
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| mask & (1 << i) != 0)
                            .map(|(_, e)| *e)
                            .collect();
                        one(self, &subset)
                    })
                    .collect();
                (groups, true)
            }
        }
    }

    /// Resolves `GROUP BY 1` and `GROUP BY alias` to the select item.
    fn normalise_key(&self, e: ExprId, items: &[Item]) -> Key {
        let hir = self.hir;
        let kind = hir.exprs.get(e).map(|x| &x.kind);
        if let Some(ExprKind::Lit(Lit::Number(sym))) = kind {
            let ordinal = hir.syms.text(*sym).parse::<usize>().ok();
            if let Some(item) = ordinal
                .and_then(|n| n.checked_sub(1))
                .and_then(|n| items.get(n))
            {
                match item {
                    Item::Expr { expr, .. } => return Key::Expr(*expr),
                    Item::Field(field) => return Key::Field(field.clone()),
                    Item::Jinja { .. } => {}
                }
            }
        }
        if let Some(ExprKind::Col(col)) = kind {
            if let Some(Resolution::LateralAlias(item)) =
                hir.cols.get(*col).and_then(|c| c.res.as_ref())
            {
                if let Some(expr) = self.env.aliases.get(*item as usize).copied().flatten() {
                    return Key::Expr(expr);
                }
            }
        }
        Key::Expr(e)
    }

    /// `GROUP BY ALL`: every item that is not an aggregate.
    fn group_all(&self, items: &[Item]) -> Vec<Key> {
        let mut keys = Vec::new();
        for item in items {
            match item {
                Item::Expr { expr, .. } => {
                    let mut calls = Calls::default();
                    self.walk_calls(*expr, 0, &mut calls);
                    if calls.aggs.is_empty() && calls.wins.is_empty() {
                        keys.push(Key::Expr(*expr));
                    }
                }
                Item::Field(field) => keys.push(Key::Field(field.clone())),
                Item::Jinja { .. } => {}
            }
        }
        keys
    }

    fn key_column(
        &mut self,
        key: &Key,
        items: &[Item],
        keys: &mut Vec<(ColId, ScalarId)>,
        used: &mut BTreeSet<ColId>,
        prov: ProvId,
    ) -> ColId {
        let hir = self.hir;
        match key {
            Key::Field(field) => {
                if used.insert(field.col) {
                    let scalar = self.col_scalar(field.col, field.ty.clone(), prov);
                    keys.push((field.col, scalar));
                    self.env.key_cols.push(field.col);
                }
                field.col
            }
            Key::Expr(expr) => {
                let known = self
                    .env
                    .keys
                    .iter()
                    .find(|(k, _, _)| same_expr(hir, *k, *expr));
                if let Some((_, col, _)) = known {
                    return *col;
                }
                let scalar = self.expr(*expr);
                let ty = self.scalar_type(scalar);
                let bare = match self.mir.scalars.get(scalar).map(|s| &s.kind) {
                    Some(ScalarKind::Col(col)) => Some(*col),
                    _ => None,
                };
                let col = match bare {
                    Some(col) if used.insert(col) => col,
                    _ => {
                        let name = item_name(hir, *expr, items);
                        self.mir.add_col(name, ty.clone())
                    }
                };
                keys.push((col, scalar));
                self.env.key_cols.push(col);
                self.env.keys.push((*expr, col, ty));
                col
            }
        }
    }

    fn agg_call(&mut self, id: ExprId) -> Option<(ColId, AggCall)> {
        let hir = self.hir;
        let earlier = self
            .env
            .aggs
            .iter()
            .find(|(seen, _)| same_expr(hir, **seen, id))
            .map(|(_, col)| *col);
        if let Some(col) = earlier {
            // HAVING and the select list often repeat one aggregate
            self.env.aggs.insert(id, col);
            return None;
        }
        let node = hir.exprs.get(id)?;
        let ExprKind::Call {
            name,
            args,
            distinct,
            filter,
            ..
        } = &node.kind
        else {
            return None;
        };
        let lowered: Vec<ScalarId> = args.iter().map(|a| self.expr(*a)).collect();
        let types: Vec<ColType> = lowered.iter().map(|a| self.scalar_type(*a)).collect();
        let filter = filter.map(|f| self.expr(f));
        let ty = infer::aggregate(hir.syms.text(*name), &types);
        let col = self.mir.add_col(None, ty);
        self.env.aggs.insert(id, col);
        Some((
            col,
            AggCall {
                func: *name,
                args: lowered,
                distinct: *distinct,
                filter,
            },
        ))
    }
}

/// The alias of the select item a key expression came from, if any.
fn item_name(hir: &cogitator_ir::hir::Hir, key: ExprId, items: &[Item]) -> Option<Symbol> {
    items.iter().find_map(|item| match item {
        Item::Expr { expr, name } if same_expr(hir, *expr, key) => *name,
        _ => None,
    })
}
