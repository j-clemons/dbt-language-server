//! Column references: `Resolution` to `ColId`, correlation bookkeeping,
//! outer-join nullability and lateral aliases.

use cogitator_ir::common::HoleKind;
use cogitator_ir::hir::{ColRefId, ExprId, FromId, Resolution};
use cogitator_ir::mir::{ColId, HirNode, ProvId, ScalarId, ScalarKind};
use cogitator_ir::{ColType, DegradationKind, Nullability, Span, Symbol};

use super::Ctx;

impl Ctx<'_> {
    pub(super) fn column(&mut self, expr: ExprId, id: ColRefId, span: Span) -> ScalarId {
        let hir = self.hir;
        let prov = self.prov(HirNode::Col(id), span);
        let Some(col) = hir.cols.get(id) else {
            self.bug(format!("expr {expr:?} reads missing col ref {id:?}"));
            return self.opaque_scalar(Vec::new(), HoleKind::Unsupported, span, prov);
        };
        let name = col.name.folded;
        match &col.res {
            Some(Resolution::Bound { from, col: index })
            | Some(Resolution::Outer {
                from, col: index, ..
            }) => match self.by_index(*from, *index as usize, name) {
                Some(col) => self.read(*from, col, span, prov),
                None => self.unbound(span, prov, "its FROM item has no such column"),
            },
            Some(Resolution::OpenSchema(from)) => match self.by_name(*from, name) {
                Some(col) => self.read(*from, col, span, prov),
                None => self.unbound(span, prov, "its open FROM item could not supply it"),
            },
            Some(Resolution::LateralAlias(item)) => self.lateral(*item, span, prov),
            Some(Resolution::Candidates(_)) => {
                self.unbound(span, prov, "it is ambiguous across open FROM items")
            }
            Some(Resolution::Unresolved) => self.unbound(span, prov, "it did not resolve"),
            None => self.unbound(span, prov, "the binder did not visit it"),
        }
    }

    fn by_index(&self, from: FromId, index: usize, name: Symbol) -> Option<ColId> {
        let binding = self.bindings.get(&from)?;
        match binding.cols.get(index) {
            Some(col) if self.col_name(*col).is_none_or(|n| n == name) => Some(*col),
            _ => self.by_name(from, name),
        }
    }

    pub(super) fn by_name(&self, from: FromId, name: Symbol) -> Option<ColId> {
        let binding = self.bindings.get(&from)?;
        binding
            .cols
            .iter()
            .copied()
            .find(|col| self.col_name(*col) == Some(name))
    }

    /// Reads `col` of `from`, as `Outer` if it is bound beyond a
    /// correlation boundary.
    fn read(&mut self, from: FromId, col: ColId, span: Span, prov: ProvId) -> ScalarId {
        let Some(binding) = self.bindings.get(&from) else {
            return self.unbound(span, prov, "its FROM item is not in scope");
        };
        let (depth, nullable) = (binding.depth, binding.nullable);
        let current = self.frames.len();
        if depth > current {
            return self.unbound(span, prov, "its FROM item is not in scope");
        }
        let ty = self.ref_type(col, nullable);
        if depth < current {
            for frame in &mut self.frames[depth..current] {
                if !frame.contains(&col) {
                    frame.push(col);
                }
            }
            return self.mir.add_scalar(ScalarKind::Outer(col), ty, prov);
        }
        if self.env.grouped && !self.env.key_cols.contains(&col) {
            return self.unbound(span, prov, "it is neither grouped nor aggregated");
        }
        self.col_scalar(col, ty, prov)
    }

    /// A column's type at a reference: outer joins force `Nullable`.
    pub(super) fn ref_type(&self, col: ColId, nullable: bool) -> ColType {
        let mut ty = self.col_type(col);
        if nullable {
            ty.null = Nullability::Nullable;
        }
        ty
    }

    /// Expands an alias in place. Each expansion allocates its own
    /// scalars and subquery relations, so operators stay trees.
    fn lateral(&mut self, item: u32, span: Span, prov: ProvId) -> ScalarId {
        let target = self.env.aliases.get(item as usize).copied().flatten();
        let Some(expr) = target else {
            return self.unbound(span, prov, "its lateral alias has no expression");
        };
        if self.env.expanding.contains(&item) {
            return self.unbound(span, prov, "its lateral alias refers to itself");
        }
        self.env.expanding.push(item);
        let scalar = self.expr(expr);
        self.env.expanding.pop();
        scalar
    }

    fn unbound(&mut self, span: Span, prov: ProvId, why: &str) -> ScalarId {
        self.degrade(
            DegradationKind::OpaqueExpression,
            span,
            format!("column reference is opaque: {why}"),
        );
        self.opaque_scalar(Vec::new(), HoleKind::Unsupported, span, prov)
    }
}
