//! Hand-built MIR for lowering, rule and oracle tests.

use crate::common::{BinaryOp, HoleKind, HoleRef, JoinKind, Lit, SetOpKind};
use crate::mir::{
    AggCall, ColId, CteDef, CteId, GroupingSets, HirNode, HirRef, Mir, Origin, ProvId, RelId,
    RelOp, ScalarId, ScalarKind, TableDef, TableId, TableRef, WinCall,
};
use crate::schema::Completeness;
use crate::{ColType, Interner, Span, Symbol};

pub struct MirBuilder {
    pub mir: Mir,
    prov: ProvId,
}

impl Default for MirBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MirBuilder {
    pub fn new() -> Self {
        let mut mir = Mir::new(Interner::new());
        let prov = mir.prov.add(Origin::Hir(HirRef {
            node: HirNode::None,
            span: Span::default(),
        }));
        Self { mir, prov }
    }

    pub fn sym(&mut self, text: &str) -> Symbol {
        self.mir.syms.intern(text)
    }

    pub fn prov(&self) -> ProvId {
        self.prov
    }

    pub fn col(&mut self, name: &str, ty: ColType) -> ColId {
        let name = self.sym(name);
        self.mir.add_col(Some(name), ty)
    }

    pub fn table(&mut self, name: &str) -> TableId {
        let name = self.sym(name);
        self.mir.tables.alloc(TableDef {
            target: TableRef::Model {
                package: None,
                name,
                version: None,
            },
            completeness: Completeness::Closed,
        })
    }

    pub fn scan(&mut self, table: &str, cols: &[(&str, ColType)]) -> RelId {
        let table = self.table(table);
        let cols = cols
            .iter()
            .map(|(name, ty)| self.col(name, ty.clone()))
            .collect();
        self.rel(RelOp::Scan {
            table,
            cols,
            open: false,
        })
    }

    pub fn rel(&mut self, op: RelOp) -> RelId {
        let prov = self.prov;
        self.mir.add_rel(op, prov)
    }

    pub fn scalar(&mut self, kind: ScalarKind, ty: ColType) -> ScalarId {
        let prov = self.prov;
        self.mir.add_scalar(kind, ty, prov)
    }

    pub fn out(&self, rel: RelId) -> Vec<ColId> {
        self.mir.rels.get(rel).map_or(Vec::new(), |r| r.out.clone())
    }

    /// A reference to `col`, typed from its definition.
    pub fn col_ref(&mut self, col: ColId) -> ScalarId {
        let ty = self
            .mir
            .cols
            .get(col)
            .map_or(ColType::UNKNOWN, |d| d.ty.clone());
        self.scalar(ScalarKind::Col(col), ty)
    }

    pub fn number(&mut self, text: &str) -> ScalarId {
        let sym = self.sym(text);
        self.scalar(ScalarKind::Lit(Lit::Number(sym)), super::int_col())
    }

    pub fn null_lit(&mut self) -> ScalarId {
        self.scalar(ScalarKind::Lit(Lit::Null), ColType::UNKNOWN)
    }

    pub fn binary(&mut self, op: BinaryOp, l: ScalarId, r: ScalarId) -> ScalarId {
        self.scalar(ScalarKind::Binary { op, l, r }, ColType::UNKNOWN)
    }

    pub fn call(&mut self, name: &str, args: Vec<ScalarId>) -> ScalarId {
        let name = self.sym(name);
        self.scalar(
            ScalarKind::Call {
                name,
                args,
                distinct: false,
            },
            ColType::UNKNOWN,
        )
    }

    pub fn filter(&mut self, input: RelId, conjuncts: Vec<ScalarId>) -> RelId {
        self.rel(RelOp::Filter { input, conjuncts })
    }

    pub fn project(&mut self, input: RelId, items: Vec<(ColId, ScalarId)>) -> RelId {
        self.rel(RelOp::Project { input, items })
    }

    pub fn join(&mut self, kind: JoinKind, l: RelId, r: RelId, on: Vec<ScalarId>) -> RelId {
        self.rel(RelOp::Join { kind, l, r, on })
    }

    pub fn aggregate(
        &mut self,
        input: RelId,
        keys: Vec<(ColId, ScalarId)>,
        aggs: Vec<(ColId, AggCall)>,
        sets: Option<GroupingSets>,
    ) -> RelId {
        self.rel(RelOp::Aggregate {
            input,
            keys,
            aggs,
            sets,
        })
    }

    pub fn window(&mut self, input: RelId, funcs: Vec<(ColId, WinCall)>) -> RelId {
        self.rel(RelOp::Window { input, funcs })
    }

    pub fn distinct(&mut self, input: RelId) -> RelId {
        self.rel(RelOp::Distinct(input))
    }

    pub fn set_op(
        &mut self,
        op: SetOpKind,
        all: bool,
        inputs: Vec<RelId>,
        out: Vec<ColId>,
    ) -> RelId {
        self.rel(RelOp::SetOp {
            op,
            all,
            inputs,
            out,
        })
    }

    pub fn opaque(&mut self, inputs: Vec<RelId>, out: Vec<ColId>, kind: HoleKind) -> RelId {
        self.rel(RelOp::Opaque {
            inputs,
            out,
            open: false,
            hole: HoleRef {
                kind,
                span: Span::default(),
            },
        })
    }

    pub fn cte(&mut self, name: &str, body: RelId) -> CteId {
        let name = Some(self.sym(name));
        let out = self.out(body);
        self.mir.ctes.alloc(CteDef { name, body, out })
    }

    /// A reference to `cte` with fresh ColIds named after its columns.
    pub fn cte_ref(&mut self, cte: CteId) -> RelId {
        let defs: Vec<ColId> = self.mir.ctes.get(cte).map_or(Vec::new(), |d| d.out.clone());
        let mut out = Vec::with_capacity(defs.len());
        for col in defs {
            if let Some(def) = self.mir.cols.get(col).cloned() {
                out.push(self.mir.add_col(def.name, def.ty));
            }
        }
        self.rel(RelOp::CteRef { cte, out })
    }

    /// Sets the root and returns the plan.
    pub fn finish(mut self, root: RelId) -> Mir {
        self.mir.root = root;
        self.mir
    }
}
