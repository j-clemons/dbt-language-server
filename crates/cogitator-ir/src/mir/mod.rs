//! MIR: normalised, typed relational algebra (specs/ir-pipeline/02).
//!
//! The arenas are append-only. A rewrite adds nodes and moves `root`,
//! so the previous plan stays intact for rollback and before/after
//! views. Only nodes reachable from `root` are "the plan": ColId
//! uniqueness and the other invariants apply to that reachable set.

pub mod ops;
pub mod print;
pub mod prov;
pub mod rel;
pub mod verify;

use serde::Serialize;

use crate::common::ModelConfig;
use crate::{Arena, ColType, Idx, Interner, Symbol};

pub use ops::{AggCall, Scalar, ScalarKind, SortKey, SubqueryKind, WinCall};
pub use print::{plan_hash, print_mir};
pub use prov::{HirNode, HirRef, Origin, ProvTable};
pub use rel::{GroupingSets, Rel, RelOp, TableDef, TableRef};
pub use verify::{verify_mir, verify_rewrite};

pub type RelId = Idx<Rel>;
pub type ScalarId = Idx<Scalar>;
pub type ColId = Idx<ColumnDef>;
pub type CteId = Idx<CteDef>;
pub type TableId = Idx<TableDef>;
pub type ProvId = Idx<Origin>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ColumnDef {
    pub name: Option<Symbol>,
    pub ty: ColType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CteDef {
    pub name: Option<Symbol>,
    pub body: RelId,
    pub out: Vec<ColId>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Mir {
    pub syms: Interner,
    pub rels: Arena<Rel>,
    pub scalars: Arena<Scalar>,
    pub cols: Arena<ColumnDef>,
    pub ctes: Arena<CteDef>,
    pub tables: Arena<TableDef>,
    pub prov: ProvTable,
    pub root: RelId,
    /// The model config lowering read; MIR→LIR builds the `Sink` from it.
    pub config: ModelConfig,
}

impl Mir {
    pub fn new(syms: Interner) -> Self {
        Self {
            syms,
            rels: Arena::new(),
            scalars: Arena::new(),
            cols: Arena::new(),
            ctes: Arena::new(),
            tables: Arena::new(),
            prov: ProvTable::default(),
            root: RelId::from_raw(0),
            config: ModelConfig::default(),
        }
    }

    pub fn add_col(&mut self, name: Option<Symbol>, ty: ColType) -> ColId {
        self.cols.alloc(ColumnDef { name, ty })
    }

    pub fn add_scalar(&mut self, kind: ScalarKind, ty: ColType, prov: ProvId) -> ScalarId {
        self.scalars.alloc(Scalar { kind, ty, prov })
    }

    /// Appends a relation whose `out` is computed from `op`.
    pub fn add_rel(&mut self, op: RelOp, prov: ProvId) -> RelId {
        let out = op.computed_output(&self.rels);
        self.rels.alloc(Rel { op, out, prov })
    }

    /// Output columns of the plan's root, empty on a bad root id.
    pub fn root_columns(&self) -> &[ColId] {
        self.rels.get(self.root).map_or(&[], |r| r.out.as_slice())
    }

    /// Relations reachable from `root`, ascending. Follows inputs,
    /// subqueries in scalars, and CTE references into CTE bodies.
    pub fn reachable(&self) -> Vec<RelId> {
        self.reachable_from(&[self.root])
    }

    pub fn reachable_from(&self, starts: &[RelId]) -> Vec<RelId> {
        let mut seen = vec![false; self.rels.len()];
        let mut stack: Vec<RelId> = starts.to_vec();
        while let Some(id) = stack.pop() {
            let Some(rel) = self.rels.get(id) else {
                continue;
            };
            if std::mem::replace(&mut seen[id.index()], true) {
                continue;
            }
            stack.extend(rel.op.inputs());
            if let RelOp::CteRef { cte, .. } = rel.op {
                if let Some(def) = self.ctes.get(cte) {
                    stack.push(def.body);
                }
            }
            for scalar in rel.op.scalars() {
                self.push_subquery_rels(scalar, &mut stack);
            }
        }
        self.rels.ids().filter(|id| seen[id.index()]).collect()
    }

    fn push_subquery_rels(&self, root: ScalarId, stack: &mut Vec<RelId>) {
        let mut scalars = vec![root];
        while let Some(id) = scalars.pop() {
            let Some(scalar) = self.scalars.get(id) else {
                continue;
            };
            if let ScalarKind::Subquery { rel, .. } = scalar.kind {
                stack.push(rel);
            }
            scalars.extend(scalar.kind.children());
        }
    }
}
