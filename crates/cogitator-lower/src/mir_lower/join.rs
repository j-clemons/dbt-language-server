//! Join conditions, USING and NATURAL, and UNNEST on the right of a join.

use cogitator_ir::common::{BinaryOp, HoleKind, JoinKind};
use cogitator_ir::hir::{FromId, FromKind, JoinCond};
use cogitator_ir::mir::{ColId, HirNode, ProvId, RelId, RelOp, ScalarId};
use cogitator_ir::{ColType, DegradationKind, Nullability, Span, Symbol};

use super::demand::leaves;
use super::{Ctx, OpaqueSpec};

impl Ctx<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn join(
        &mut self,
        id: FromId,
        kind: JoinKind,
        l: FromId,
        r: FromId,
        cond: &JoinCond,
        nullable: bool,
        prov: ProvId,
    ) -> RelId {
        let hir = self.hir;
        let span = hir.from.get(id).map_or(Span::default(), |f| f.span);
        let left_null = nullable || matches!(kind, JoinKind::Right | JoinKind::Full);
        let right_null = nullable || matches!(kind, JoinKind::Left | JoinKind::Full);
        let left = self.from(l, left_null);
        if hir
            .from
            .get(r)
            .is_some_and(|f| matches!(f.kind, FromKind::Unnest { .. }))
        {
            return self.join_unnest(left, r, kind, cond, right_null, span);
        }
        let right = self.from(r, right_null);
        let on = match cond {
            JoinCond::None => Some(Vec::new()),
            JoinCond::On(e) => Some(self.conjuncts(*e)),
            JoinCond::Using(names) => {
                let names: Vec<Symbol> = names.iter().map(|n| n.folded).collect();
                self.equi_pairs(l, r, &names, prov)
            }
            JoinCond::Natural => self
                .natural_names(l, r)
                .and_then(|names| self.equi_pairs(l, r, &names, prov)),
        };
        let Some(on) = on else {
            self.degrade(
                DegradationKind::OpenSchema,
                span,
                "the join columns are not known for this USING or NATURAL join",
            );
            return self.opaque_join(&[left, right], &[l, r], span, prov);
        };
        self.mir.add_rel(
            RelOp::Join {
                kind,
                l: left,
                r: right,
                on,
            },
            prov,
        )
    }

    /// `l.x = r.x` per name, or `None` when a side cannot supply one.
    fn equi_pairs(
        &mut self,
        l: FromId,
        r: FromId,
        names: &[Symbol],
        prov: ProvId,
    ) -> Option<Vec<ScalarId>> {
        let mut on = Vec::with_capacity(names.len());
        for name in names {
            let (lc, ln) = self.find_in(l, *name)?;
            let (rc, rn) = self.find_in(r, *name)?;
            let (lt, rt) = (self.ref_type(lc, ln), self.ref_type(rc, rn));
            let (ls, rs) = (self.col_scalar(lc, lt, prov), self.col_scalar(rc, rt, prov));
            let ty = ColType::new(cogitator_ir::SqlType::Bool, Nullability::Unknown);
            let kind = cogitator_ir::mir::ScalarKind::Binary {
                op: BinaryOp::Eq,
                l: ls,
                r: rs,
            };
            on.push(self.mir.add_scalar(kind, ty, prov));
        }
        Some(on)
    }

    /// The first column called `name` under `from`, and its nullability.
    pub(super) fn find_in(&self, from: FromId, name: Symbol) -> Option<(ColId, bool)> {
        leaves(self.hir, from).into_iter().find_map(|leaf| {
            let col = self.by_name(leaf, name)?;
            Some((col, self.bindings.get(&leaf)?.nullable))
        })
    }

    /// The names both sides share; `None` if either side is open, since
    /// the shared set is then unknown.
    pub(super) fn natural_names(&self, l: FromId, r: FromId) -> Option<Vec<Symbol>> {
        let hir = self.hir;
        let open = |side: FromId| {
            leaves(hir, side)
                .iter()
                .any(|leaf| hir.from.get(*leaf).is_some_and(|item| item.open))
        };
        if open(l) || open(r) {
            return None;
        }
        let names = |side: FromId| -> Vec<Symbol> {
            leaves(hir, side)
                .iter()
                .filter_map(|leaf| self.bindings.get(leaf))
                .flat_map(|b| b.cols.iter().filter_map(|c| self.col_name(*c)))
                .collect()
        };
        let right = names(r);
        let mut shared: Vec<Symbol> = Vec::new();
        for name in names(l) {
            if right.contains(&name) && !shared.contains(&name) {
                shared.push(name);
            }
        }
        Some(shared)
    }

    /// Replaces a join it cannot model; every leaf rebinds to fresh
    /// columns the `Opaque` defines, so references above stay visible.
    fn opaque_join(
        &mut self,
        inputs: &[RelId],
        sides: &[FromId],
        span: Span,
        prov: ProvId,
    ) -> RelId {
        let hir = self.hir;
        let all: Vec<FromId> = sides.iter().flat_map(|side| leaves(hir, *side)).collect();
        let mut cols = Vec::new();
        for leaf in &all {
            if let Some(binding) = self.bindings.get(leaf) {
                cols.extend(
                    binding
                        .cols
                        .iter()
                        .map(|c| (self.col_name(*c), self.col_type(*c))),
                );
            }
        }
        let spec = OpaqueSpec {
            cols,
            open: true,
            kind: HoleKind::Unsupported,
            span,
        };
        let (rel, fresh) = self.opaque_rel(inputs.to_vec(), spec, prov);
        let mut fresh = fresh.into_iter();
        for leaf in all {
            if let Some(binding) = self.bindings.get_mut(&leaf) {
                binding.cols = fresh.by_ref().take(binding.cols.len()).collect();
            }
        }
        rel
    }

    fn join_unnest(
        &mut self,
        left: RelId,
        r: FromId,
        kind: JoinKind,
        cond: &JoinCond,
        nullable: bool,
        span: Span,
    ) -> RelId {
        let rel = self.unnest(left, r, kind, nullable);
        match (kind, cond) {
            (_, JoinCond::None) => rel,
            (JoinKind::Inner | JoinKind::Cross, JoinCond::On(e)) => {
                let conjuncts = self.conjuncts(*e);
                let prov = self.prov(HirNode::From(r), span);
                self.mir.add_rel(
                    RelOp::Filter {
                        input: rel,
                        conjuncts,
                    },
                    prov,
                )
            }
            _ => {
                self.degrade(
                    DegradationKind::UnsupportedSyntax,
                    span,
                    "the join condition of this UNNEST is not represented",
                );
                rel
            }
        }
    }

    /// `Unnest` is a lateral cross product over `input`. Row preservation
    /// of `LEFT JOIN UNNEST` has no MIR form; it is reported, and the
    /// new columns are typed nullable.
    pub(super) fn unnest(
        &mut self,
        input: RelId,
        id: FromId,
        kind: JoinKind,
        nullable: bool,
    ) -> RelId {
        let hir = self.hir;
        let Some(item) = hir.from.get(id) else {
            return input;
        };
        let FromKind::Unnest { exprs, .. } = &item.kind else {
            return input;
        };
        let prov = self.prov(HirNode::From(id), item.span);
        if !matches!(kind, JoinKind::Inner | JoinKind::Cross) {
            self.degrade(
                DegradationKind::UnsupportedSyntax,
                item.span,
                "an outer join to UNNEST is modelled as a cross product",
            );
        }
        let exprs = exprs.iter().map(|e| self.expr(*e)).collect();
        let out: Vec<ColId> = self
            .scan_columns(id, item)
            .into_iter()
            .map(|(name, mut ty)| {
                if nullable {
                    ty.null = Nullability::Nullable;
                }
                self.mir.add_col(name, ty)
            })
            .collect();
        let rel = self.mir.add_rel(
            RelOp::Unnest {
                input,
                exprs,
                out: out.clone(),
            },
            prov,
        );
        self.bind(id, out, nullable);
        rel
    }
}
