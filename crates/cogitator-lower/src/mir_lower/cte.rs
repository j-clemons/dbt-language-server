//! CTE definitions: column lists, correlation, and recursive CTEs.

use cogitator_ir::common::HoleKind;
use cogitator_ir::hir::{Body, Cte, CteId};
use cogitator_ir::mir::{CteDef, HirNode, ProvId, RelOp};
use cogitator_ir::{ColType, DegradationKind, Symbol};

use super::query::QueryOut;
use super::{Ctx, OpaqueSpec};

impl Ctx<'_> {
    pub(super) fn cte(&mut self, id: CteId) {
        let hir = self.hir;
        let Some(cte) = hir.ctes.get(id) else {
            self.bug(format!("CTE {id:?} is out of range"));
            return;
        };
        let prov = self.prov(HirNode::Query(cte.query), cte.span);
        // boundary: the demand list outlives the mutable walk below
        let demand: Vec<Symbol> = self.demand.for_cte(id).to_vec();
        let out = if cte.recursive {
            self.recursive_cte(cte, &demand, prov)
        } else {
            let out = self.isolated(cte.query, &demand, cte.span, prov);
            self.rename(out, cte, prov)
        };
        let def = self.mir.ctes.alloc(CteDef {
            name: Some(cte.name.folded),
            body: out.rel,
            out: out.cols,
        });
        self.ctes.insert(id, def);
    }

    /// `Opaque`, keeping the anchor as an input; the recursive term is
    /// not lowered, so the CTE graph stays acyclic.
    fn recursive_cte(&mut self, cte: &Cte, demand: &[Symbol], prov: ProvId) -> QueryOut {
        let hir = self.hir;
        self.degrade(
            DegradationKind::RecursiveCte,
            cte.span,
            "a recursive CTE is opaque to the plan",
        );
        let anchor = match hir.queries.get(cte.query).map(|q| &q.body) {
            Some(Body::SetOp { l, .. }) => Some(self.isolated(*l, &[], cte.span, prov)),
            _ => None,
        };
        let mut cols: Vec<(Option<Symbol>, ColType)> = match &anchor {
            Some(a) => self.spec_of(&a.cols),
            None => Vec::new(),
        };
        for (i, name) in cte.columns.iter().enumerate() {
            match cols.get_mut(i) {
                Some(slot) => slot.0 = Some(name.folded),
                None => cols.push((Some(name.folded), ColType::UNKNOWN)),
            }
        }
        for name in demand {
            if !cols.iter().any(|(n, _)| *n == Some(*name)) {
                cols.push((Some(*name), ColType::UNKNOWN));
            }
        }
        let open = anchor.as_ref().is_none_or(|a| a.open);
        let inputs = anchor.iter().map(|a| a.rel).collect();
        let spec = OpaqueSpec {
            cols,
            open,
            kind: HoleKind::Unsupported,
            span: cte.span,
        };
        let (rel, cols) = self.opaque_rel(inputs, spec, prov);
        QueryOut { rel, cols, open }
    }

    /// Applies `WITH c(a, b)` names with a Project of fresh columns.
    fn rename(&mut self, out: QueryOut, cte: &Cte, prov: ProvId) -> QueryOut {
        if cte.columns.is_empty() {
            return out;
        }
        let mut items = Vec::with_capacity(out.cols.len());
        for (i, col) in out.cols.iter().enumerate() {
            let name = cte
                .columns
                .get(i)
                .map(|n| n.folded)
                .or_else(|| self.col_name(*col));
            let ty = self.col_type(*col);
            let fresh = self.mir.add_col(name, ty.clone());
            items.push((fresh, self.col_scalar(*col, ty, prov)));
        }
        let cols = items.iter().map(|(c, _)| *c).collect();
        let rel = self.mir.add_rel(
            RelOp::Project {
                input: out.rel,
                items,
            },
            prov,
        );
        QueryOut {
            rel,
            cols,
            open: out.open,
        }
    }
}
