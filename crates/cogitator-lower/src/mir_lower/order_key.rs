//! Resolving one ORDER BY key against the select list.

use cogitator_ir::common::Lit;
use cogitator_ir::hir::{ExprId, ExprKind, OrderItem, Resolution};
use cogitator_ir::mir::ScalarId;
use cogitator_ir::{DegradationKind, Span};

use super::equal::same_expr;
use super::star::{Expanded, Item};
use super::Ctx;

/// Where one ORDER BY key reads from.
pub(super) enum SortSource {
    Output(usize),
    Hidden(ScalarId),
}

pub(super) struct Planned {
    pub(super) source: SortSource,
    pub(super) desc: Option<bool>,
    pub(super) nulls_first: Option<bool>,
}

impl Ctx<'_> {
    pub(super) fn plan_key(
        &mut self,
        key: &OrderItem,
        expanded: &Expanded,
        distinct: bool,
    ) -> Option<Planned> {
        let hir = self.hir;
        let span = hir.exprs.get(key.expr).map_or(Span::default(), |e| e.span);
        let source = match self.output_of(key.expr, expanded) {
            Some(i) => SortSource::Output(i),
            None if distinct => {
                self.degrade(
                    DegradationKind::UnsupportedSyntax,
                    span,
                    "this ORDER BY key is not in the DISTINCT output",
                );
                return None;
            }
            None => SortSource::Hidden(self.expr(key.expr)),
        };
        Some(Planned {
            source,
            desc: key.desc,
            nulls_first: key.nulls_first,
        })
    }

    /// The output position an ORDER BY key names: an ordinal, a lateral
    /// alias, or an expression equal to a select item.
    fn output_of(&self, key: ExprId, expanded: &Expanded) -> Option<usize> {
        let hir = self.hir;
        match hir.exprs.get(key).map(|e| &e.kind) {
            Some(ExprKind::Lit(Lit::Number(sym))) => {
                let n = hir.syms.text(*sym).parse::<usize>().ok()?.checked_sub(1)?;
                return (n < expanded.items.len()).then_some(n);
            }
            Some(ExprKind::Col(col)) => {
                let col = hir.cols.get(*col)?;
                if let Some(Resolution::LateralAlias(item)) = &col.res {
                    return expanded.item_of.get(*item as usize).copied().flatten();
                }
                if let Some(Resolution::Bound { from, col: index }) = &col.res {
                    let target = self.bindings.get(from)?.cols.get(*index as usize).copied();
                    let hit = expanded.items.iter().position(
                        |item| matches!(item, Item::Field(field) if Some(field.col) == target),
                    );
                    if hit.is_some() {
                        return hit;
                    }
                }
            }
            _ => {}
        }
        expanded
            .items
            .iter()
            .position(|item| matches!(item, Item::Expr { expr, .. } if same_expr(hir, *expr, key)))
    }
}
